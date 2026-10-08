//! Transport types used by the receiver service.

use std::{
    io,
    net::{IpAddr, SocketAddr, SocketAddrV4, SocketAddrV6},
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
            let stream = GuardedIo::new(stream, cancel.clone(), permit);
            return (
                SinkWriter::new(StreamReader::new(Framed::new(
                    stream,
                    UpgradeableCodec::new(
                        codec::Rtsp2Http::default(),
                        codec::Rtsp2Http::default(),
                        session_key.clone(),
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
    _permit: tokio::sync::OwnedSemaphorePermit,
}
impl GuardedIo {
    fn new(
        stream: TcpStream,
        cancel: tokio_util::sync::CancellationToken,
        permit: tokio::sync::OwnedSemaphorePermit,
    ) -> Self {
        Self {
            stream,
            cancelled: Box::pin(cancel.cancelled_owned()),
            idle: Box::pin(tokio::time::sleep(std::time::Duration::from_secs(30))),
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
