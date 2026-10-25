//! Transport types used by the receiver service.

use std::{
    io,
    net::{IpAddr, SocketAddr, SocketAddrV4, SocketAddrV6},
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::serve::Listener;
use tokio::{
    io::Result,
    net::{TcpSocket, TcpStream},
};
use tokio_dual_stack::{DualStackTcpListener, Tcp as _};
use tokio_util::{
    codec::{Decoder, Framed},
    io::{SinkWriter, StreamReader},
};

use crate::pairing::{SharedSessionKey, codec::UpgradeableCodec};

pub(crate) mod codec;

/// A request has one absolute budget, including encrypted fragments and its body.
/// The I/O owner polls its timer, so a partial request expires even without another byte.
#[derive(Clone)]
pub(crate) struct RequestBudget {
    deadline: Arc<Mutex<Option<tokio::time::Instant>>>,
    timeout: Duration,
}
impl Default for RequestBudget {
    fn default() -> Self {
        Self::new(Duration::from_secs(15))
    }
}
impl RequestBudget {
    fn new(timeout: Duration) -> Self {
        Self {
            deadline: Arc::new(Mutex::new(None)),
            timeout,
        }
    }
    pub(crate) fn begin(&self) -> io::Result<()> {
        let mut deadline = self.deadline.lock().unwrap_or_else(|e| e.into_inner());
        let now = tokio::time::Instant::now();
        if deadline.is_some_and(|end| now >= end) {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "RTSP request deadline",
            ));
        }
        deadline.get_or_insert(now + self.timeout);
        Ok(())
    }
    pub(crate) fn finish(&self) {
        *self.deadline.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
    fn deadline(&self) -> Option<tokio::time::Instant> {
        *self.deadline.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Dual-stack listener used by the receiver service.
///
/// This accepts IPv4 and IPv6 TCP connections on the same port and wraps them
/// in the codec stack expected by the RTSP service.
pub struct DualStackListenerWithRtspRemap {
    listener: DualStackTcpListener,
    bind_addr4: SocketAddrV4,
    bind_addr6: SocketAddrV6,
    slots: std::sync::Arc<tokio::sync::Semaphore>,
    next_id: u64,
}

/// Metadata attached to an accepted connection.
#[derive(Debug, Clone)]
pub struct Connection {
    pub id: u64,
    pub cancel: tokio_util::sync::CancellationToken,
    /// Listener IPv4 bind address.
    pub bind_addr4: SocketAddrV4,
    /// Listener IPv6 bind address.
    pub bind_addr6: SocketAddrV6,
    /// Socket address accepted on this machine.
    pub local_addr: SocketAddr,
    /// Remote peer socket address.
    pub remote_addr: SocketAddr,
    /// Shared session key storage used during pairing and upgrades.
    pub session_key: SharedSessionKey,
}

impl Connection {
    /// Returns the local bind IP matching the peer's address family.
    pub fn bind_addr(&self) -> IpAddr {
        match self.remote_addr {
            SocketAddr::V4(_) => IpAddr::V4(*self.bind_addr4.ip()),
            SocketAddr::V6(_) => IpAddr::V6(*self.bind_addr6.ip()),
        }
    }
}

impl DualStackListenerWithRtspRemap {
    /// Binds IPv4 and IPv6 listeners that share the same port.
    ///
    /// The IPv6 socket is opened with `IPV6_V6ONLY` enabled so it does not
    /// consume the IPv4 address space on platforms where dual-stack sockets do
    /// that by default.
    pub fn bind(addr4: SocketAddrV4, addr6: SocketAddrV6) -> io::Result<Self> {
        let ip4 = TcpSocket::new_v4()?;
        ip4.set_reuseaddr(true)?;
        ip4.bind(SocketAddr::V4(addr4))?;
        let SocketAddr::V4(addr4) = ip4.local_addr()? else {
            unreachable!()
        };
        let mut addr6 = addr6;
        addr6.set_port(addr4.port());
        // Create IPv6 socket with IPV6_V6ONLY=true so it doesn't claim the
        // IPv4 address space. Linux/Android default IPV6_V6ONLY=0 causes the
        // subsequent IPv4 bind to fail with EADDRINUSE when both sockets bind
        // to the same port.
        let ip6_raw = socket2::Socket::new(
            socket2::Domain::IPV6,
            socket2::Type::STREAM,
            Some(socket2::Protocol::TCP),
        )?;
        ip6_raw.set_only_v6(true)?;
        ip6_raw.set_reuse_address(true)?;
        ip6_raw.set_nonblocking(true)?;
        ip6_raw.bind(&SocketAddr::V6(addr6).into())?;
        let ip6 = TcpSocket::from_std_stream(std::net::TcpStream::from(ip6_raw));

        Ok(Self {
            listener: DualStackTcpListener::from_sockets((ip6, 1024), (ip4, 1024))?,
            bind_addr4: addr4,
            bind_addr6: addr6,
            slots: std::sync::Arc::new(tokio::sync::Semaphore::new(8)),
            next_id: 1,
        })
    }
    pub fn port(&self) -> u16 {
        self.bind_addr4.port()
    }
}

impl Listener for DualStackListenerWithRtspRemap {
    // TODO : TAIT upon it
    // type Io = impl AsyncRead + AsyncWrite;
    type Io = SinkWriter<
        StreamReader<
            Framed<GuardedIo, UpgradeableCodec<codec::Rtsp2Http, codec::Rtsp2Http>>,
            <UpgradeableCodec<codec::Rtsp2Http, codec::Rtsp2Http> as Decoder>::Item,
        >,
    >;
    type Addr = Connection;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            let (stream, remote_addr) = match self.listener.accept().await {
                Ok(res) => res,
                Err(err) => {
                    tracing::error!(%err, "couldn't accept connection");
                    continue;
                }
            };
            let local_addr = match stream.local_addr() {
                Ok(res) => res,
                Err(err) => {
                    tracing::error!(%err, "couldn't get local addr of connection");
                    continue;
                }
            };

            let session_key = SharedSessionKey::default();
            let Ok(permit) = self.slots.clone().try_acquire_owned() else {
                continue;
            };
            let cancel = tokio_util::sync::CancellationToken::new();
            let id = self.next_id;
            self.next_id += 1;
            let budget = RequestBudget::default();
            let stream = GuardedIo::new(
                stream,
                cancel.clone(),
                permit,
                session_key.clone(),
                budget.clone(),
            );
            return (
                SinkWriter::new(StreamReader::new(Framed::new(
                    stream,
                    UpgradeableCodec::with_budget(
                        codec::Rtsp2Http::default(),
                        codec::Rtsp2Http::default(),
                        session_key.clone(),
                        budget,
                    ),
                ))),
                Connection {
                    id,
                    cancel,
                    session_key,
                    local_addr,
                    remote_addr,
                    bind_addr4: self.bind_addr4,
                    bind_addr6: self.bind_addr6,
                },
            );
        }
    }

    fn local_addr(&self) -> Result<Self::Addr> {
        Ok(Connection {
            id: 0,
            cancel: tokio_util::sync::CancellationToken::new(),
            bind_addr4: self.bind_addr4,
            bind_addr6: self.bind_addr6,
            local_addr: SocketAddr::V4(self.bind_addr4),
            remote_addr: SocketAddr::V4(self.bind_addr4),
            session_key: SharedSessionKey::default(),
        })
    }
}

/// Bounded connection ownership and interruptible I/O, including stalled clients.
pub struct GuardedIo {
    stream: TcpStream,
    cancelled: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>,
    idle: std::pin::Pin<Box<tokio::time::Sleep>>,
    pairing: std::pin::Pin<Box<tokio::time::Sleep>>,
    request: std::pin::Pin<Box<tokio::time::Sleep>>,
    session_key: SharedSessionKey,
    budget: RequestBudget,
    _permit: tokio::sync::OwnedSemaphorePermit,
}
impl GuardedIo {
    fn new(
        stream: TcpStream,
        cancel: tokio_util::sync::CancellationToken,
        permit: tokio::sync::OwnedSemaphorePermit,
        session_key: SharedSessionKey,
        budget: RequestBudget,
    ) -> Self {
        Self {
            stream,
            cancelled: Box::pin(cancel.cancelled_owned()),
            idle: Box::pin(tokio::time::sleep(std::time::Duration::from_secs(30))),
            pairing: Box::pin(tokio::time::sleep(Duration::from_secs(60))),
            request: Box::pin(tokio::time::sleep(budget.timeout)),
            session_key,
            budget,
            _permit: permit,
        }
    }
}
impl tokio::io::AsyncRead for GuardedIo {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.cancelled.as_mut().poll(cx).is_ready() || this.idle.as_mut().poll(cx).is_ready() {
            return std::task::Poll::Ready(Ok(()));
        }
        if this.session_key.read().is_none() && this.pairing.as_mut().poll(cx).is_ready() {
            return std::task::Poll::Ready(Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "pairing deadline",
            )));
        }
        if let Some(deadline) = this.budget.deadline() {
            this.request.as_mut().reset(deadline);
            if this.request.as_mut().poll(cx).is_ready() {
                return std::task::Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "RTSP request deadline",
                )));
            }
        }
        let before = buf.filled().len();
        let result = std::pin::Pin::new(&mut this.stream).poll_read(cx, buf);
        if buf.filled().len() > before {
            this.idle
                .as_mut()
                .reset(tokio::time::Instant::now() + std::time::Duration::from_secs(30));
        }
        result
    }
}
impl tokio::io::AsyncWrite for GuardedIo {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        let this = self.get_mut();
        if this.cancelled.as_mut().poll(cx).is_ready() {
            return std::task::Poll::Ready(Err(io::Error::from(io::ErrorKind::ConnectionAborted)));
        }
        std::pin::Pin::new(&mut this.stream).poll_write(cx, buf)
    }
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut self.get_mut().stream).poll_flush(cx)
    }
    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut self.get_mut().stream).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{SinkExt, StreamExt};
    use tokio::io::AsyncWriteExt;

    async fn framed_pair(
        timeout: Duration,
    ) -> (
        TcpStream,
        Framed<GuardedIo, UpgradeableCodec<codec::Rtsp2Http, codec::Rtsp2Http>>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (server, _) = listener.accept().await.unwrap();
        let permit = Arc::new(tokio::sync::Semaphore::new(1))
            .acquire_owned()
            .await
            .unwrap();
        let key = SharedSessionKey::default();
        let budget = RequestBudget::new(timeout);
        let io = GuardedIo::new(
            server,
            tokio_util::sync::CancellationToken::new(),
            permit,
            key.clone(),
            budget.clone(),
        );
        let codec = UpgradeableCodec::with_budget(
            codec::Rtsp2Http::default(),
            codec::Rtsp2Http::default(),
            key,
            budget,
        );
        (client, Framed::new(io, codec))
    }

    #[tokio::test]
    async fn partial_request_expires_while_bytes_keep_arriving() {
        let (mut client, mut framed) = framed_pair(Duration::from_millis(60)).await;
        let writer = tokio::spawn(async move {
            for _ in 0..30 {
                if client.write_all(b"P").await.is_err() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });
        let result = tokio::time::timeout(Duration::from_secs(1), framed.next())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
        writer.abort();
        let _ = writer.await;
    }

    #[tokio::test]
    async fn completed_request_does_not_expire_the_next_request() {
        let (mut client, mut framed) = framed_pair(Duration::from_millis(60)).await;
        let request = b"OPTIONS * RTSP/1.0\r\nCSeq: 1\r\n\r\n";
        client.write_all(request).await.unwrap();
        assert!(
            framed
                .next()
                .await
                .unwrap()
                .unwrap()
                .starts_with(b"OPTIONS / HTTP/1.1")
        );
        tokio::time::sleep(Duration::from_millis(80)).await;
        client.write_all(request).await.unwrap();
        assert!(
            framed
                .next()
                .await
                .unwrap()
                .unwrap()
                .starts_with(b"OPTIONS / HTTP/1.1")
        );
    }

    #[tokio::test]
    async fn partial_header_and_body_expire_without_more_input() {
        for prefix in [
            b"OPTIONS * RTSP/1.0\r\nCSeq:".as_slice(),
            b"POST /pair-setup RTSP/1.0\r\nContent-Length: 10\r\n\r\nx".as_slice(),
        ] {
            let (mut client, mut framed) = framed_pair(Duration::from_millis(60)).await;
            client.write_all(prefix).await.unwrap();
            let result = tokio::time::timeout(Duration::from_secs(1), framed.next())
                .await
                .expect("request deadline did not wake an idle socket")
                .unwrap();
            assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
        }
    }

    #[tokio::test]
    async fn pipelined_partial_request_gets_its_own_deadline() {
        let (mut client, mut framed) = framed_pair(Duration::from_millis(60)).await;
        client
            .write_all(b"OPTIONS * RTSP/1.0\r\nCSeq: 1\r\n\r\nPOST /pair-setup RTSP/1.0\r\nContent-Length: 10\r\n\r\nx")
            .await
            .unwrap();
        assert!(framed.next().await.unwrap().is_ok());
        let result = tokio::time::timeout(Duration::from_secs(1), framed.next())
            .await
            .expect("pipelined partial request had no timer")
            .unwrap();
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
    }

    #[tokio::test]
    async fn pairing_budget_is_absolute_but_no_longer_applies_to_verified_peers() {
        for verified in [false, true] {
            let (mut client, mut framed) = framed_pair(Duration::from_secs(1)).await;
            if verified {
                framed
                    .get_mut()
                    .session_key
                    .lock_write()
                    .replace(crate::pairing::SessionKey {
                        key_material: [7; 32],
                        upgrade_channel: false,
                    });
            }
            framed
                .get_mut()
                .pairing
                .as_mut()
                .reset(tokio::time::Instant::now() + Duration::from_millis(60));
            let request = b"OPTIONS * RTSP/1.0\r\nCSeq: 1\r\n\r\n";
            client.write_all(request).await.unwrap();
            assert!(framed.next().await.unwrap().is_ok());
            tokio::time::sleep(Duration::from_millis(80)).await;
            client.write_all(request).await.unwrap();
            let result = tokio::time::timeout(Duration::from_secs(1), framed.next())
                .await
                .unwrap()
                .unwrap();
            if verified {
                assert!(result.is_ok());
            } else {
                assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
            }
        }
    }

    fn client_encrypted_block(secret: &[u8], bytes: &[u8]) -> Vec<u8> {
        use chacha20poly1305::{ChaCha20Poly1305, KeyInit, Nonce, aead::AeadInOut};
        let key = crate::crypto::hkdf(secret, b"Control-Salt", b"Control-Write-Encryption-Key");
        let length = u16::try_from(bytes.len()).unwrap().to_le_bytes();
        let mut encrypted = bytes.to_vec();
        ChaCha20Poly1305::new(&key.into())
            .encrypt_in_place(&Nonce::from([0; 12]), &length, &mut encrypted)
            .unwrap();
        [length.as_slice(), &encrypted].concat()
    }

    #[tokio::test]
    async fn upgraded_encrypted_fragments_share_the_request_deadline() {
        for complete_encrypted_block in [false, true] {
            let (mut client, mut framed) = framed_pair(Duration::from_millis(60)).await;
            let secret = [7; 32];
            framed
                .get_mut()
                .session_key
                .lock_write()
                .replace(crate::pairing::SessionKey {
                    key_material: secret,
                    upgrade_channel: true,
                });
            // The successful pairing response is plaintext; only its complete
            // response boundary switches both directions to HAP encryption.
            framed
                .send(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".as_slice())
                .await
                .unwrap();
            let block = client_encrypted_block(&secret, b"OPTIONS * RTSP/1.0\r\nCSeq:");
            let bytes = if complete_encrypted_block {
                &block[..]
            } else {
                &block[..1]
            };
            client.write_all(bytes).await.unwrap();
            let result = tokio::time::timeout(Duration::from_secs(1), framed.next())
                .await
                .expect("encrypted partial request had no timer")
                .unwrap();
            assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
        }
    }
}
