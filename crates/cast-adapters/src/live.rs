//! Nonblocking fan-out. A slow subscriber is disconnected, never fed a broken GOP.
use crate::{
    http_server::{self, Body, empty},
    ts::{self, StartGate},
};
use anyhow::ensure;
use bytes::Bytes;
use futures_util::stream;
use http_body_util::{BodyExt, StreamBody};
use hyper::{
    Request, Response,
    body::{Frame, Incoming},
    header,
};
use std::{
    net::{IpAddr, SocketAddr},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{net::TcpListener, sync::mpsc};
use tokio_util::sync::CancellationToken;
struct Reader {
    tx: mpsc::Sender<(Instant, Bytes)>,
    stop: CancellationToken,
}
pub struct LiveResource {
    token: String,
    allowed: IpAddr,
    stop: CancellationToken,
    readers: Mutex<Vec<Reader>>,
    pulls: AtomicU64,
}
impl LiveResource {
    pub fn new(allowed: IpAddr) -> Arc<Self> {
        Arc::new(Self {
            token: crate::auth::random_token()
                .replace('/', "_")
                .replace('+', "-"),
            allowed,
            stop: CancellationToken::new(),
            readers: Mutex::new(Vec::new()),
            pulls: AtomicU64::new(0),
        })
    }
    pub fn revoke(&self) {
        self.stop.cancel();
        if let Ok(mut readers) = self.readers.lock() {
            for r in readers.drain(..) {
                r.stop.cancel();
            }
        }
    }
    pub fn pulls(&self) -> u64 {
        self.pulls.load(Ordering::Acquire)
    }
    pub fn write(&self, data: &[u8]) -> anyhow::Result<()> {
        ensure!(!self.stop.is_cancelled(), "RESOURCE_CLOSED");
        ts::validate(data)?;
        let block = Bytes::copy_from_slice(data);
        let now = Instant::now();
        self.readers
            .lock()
            .map_err(|_| anyhow::anyhow!("RESOURCE_CLOSED"))?
            .retain(|r| {
                if r.stop.is_cancelled() || r.tx.try_send((now, block.clone())).is_err() {
                    r.stop.cancel();
                    false
                } else {
                    true
                }
            });
        Ok(())
    }
    fn subscribe(
        &self,
        connection: Option<CancellationToken>,
    ) -> (mpsc::Receiver<(Instant, Bytes)>, CancellationToken) {
        // 16 * 65424 = 1046784 bytes, below the per-reader 1 MiB budget.
        let (tx, rx) = mpsc::channel(16);
        let stop = connection.unwrap_or_else(|| self.stop.child_token());
        let mut readers = self.readers.lock().unwrap();
        readers.retain(|r| !r.tx.is_closed() && !r.stop.is_cancelled());
        readers.push(Reader {
            tx,
            stop: stop.clone(),
        });
        (rx, stop)
    }
    async fn respond(self: Arc<Self>, request: Request<Incoming>) -> Response<Body> {
        if self.stop.is_cancelled()
            || request.uri().query().is_some()
            || !crate::auth::same_secret(request.uri().path(), &format!("/live/{}.ts", self.token))
        {
            return empty(404);
        }
        let head = request.method() == hyper::Method::HEAD;
        if request.method() != hyper::Method::GET && !head {
            return empty(405);
        }
        let mut response = empty(200);
        response
            .headers_mut()
            .insert(header::CONTENT_TYPE, "video/mpeg".parse().unwrap());
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
        response
            .headers_mut()
            .insert(header::CONNECTION, "close".parse().unwrap());
        // Unknown-length body: Hyper emits chunked for HTTP/1.1, close-delimited for HTTP/1.0.
        if head {
            response.headers_mut().remove(header::CONTENT_LENGTH);
            *response.body_mut() =
                StreamBody::new(stream::empty::<Result<Frame<Bytes>, std::io::Error>>())
                    .boxed_unsync();
            return response;
        }
        self.pulls.fetch_add(1, Ordering::Release);
        let (rx, stop) = self.subscribe(request.extensions().get::<CancellationToken>().cloned());
        let state = (
            rx,
            stop,
            StartGate::default(),
            Instant::now(),
            Instant::now(),
            false,
        );
        let chunks = stream::try_unfold(
            state,
            |(mut rx, stop, mut gate, started, mut gate_started, mut ready)| async move {
                loop {
                    let wait = if ready {
                        Duration::from_secs(2)
                    } else {
                        Duration::from_secs(8).saturating_sub(started.elapsed())
                    };
                    let message = tokio::select! { biased;
                        _ = stop.cancelled() => return Err(std::io::Error::other("STREAM_REVOKED_OR_SLOW_READER")),
                        r = tokio::time::timeout(wait, rx.recv()) => r,
                    };
                    let (timestamp, data) = message
                        .map_err(|_| std::io::Error::other("STREAM_TIMEOUT"))?
                        .ok_or_else(|| std::io::Error::other("STREAM_CLOSED"))?;
                    if timestamp.elapsed() > Duration::from_secs(2) {
                        return Err(std::io::Error::other("SLOW_READER"));
                    }
                    if !ready && gate_started.elapsed() > Duration::from_secs(2) {
                        gate = StartGate::default();
                        gate_started = Instant::now();
                    }
                    if let Some(bytes) = gate.push(&data).map_err(std::io::Error::other)? {
                        ready = true;
                        return Ok(Some((
                            Frame::data(Bytes::from(bytes)),
                            (rx, stop, gate, started, gate_started, ready),
                        )));
                    }
                    // Do not accumulate more than two seconds of undecodable content.
                }
            },
        );
        *response.body_mut() = StreamBody::new(chunks).boxed_unsync();
        response
    }
    pub async fn bind(
        self: Arc<Self>,
        address: SocketAddr,
    ) -> anyhow::Result<(String, tokio::task::JoinHandle<()>)> {
        ensure!(!address.ip().is_unspecified(), "SELECT_LAN_INTERFACE");
        let listener = TcpListener::bind(address).await?;
        let url = format!("http://{}/live/{}.ts", listener.local_addr()?, self.token);
        let handler = self.clone();
        let task = tokio::spawn(async move {
            let _ = http_server::serve(listener, self.allowed, None, self.stop.clone(), move |r| {
                handler.clone().respond(r)
            })
            .await;
        });
        Ok((url, task))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn head_never_claims_a_finite_live_length() {
        let live = LiveResource::new("127.0.0.1".parse().unwrap());
        let (url, task) = live
            .clone()
            .bind("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let head = client.head(&url).send().await.unwrap();
        assert_eq!(head.status(), 200);
        assert!(!head.headers().contains_key("content-length"));
        assert_eq!(head.headers()["content-type"], "video/mpeg");
        assert_eq!(live.pulls(), 0);
        assert_eq!(
            client
                .get(format!("{url}?token=wrong"))
                .send()
                .await
                .unwrap()
                .status(),
            404
        );
        live.revoke();
        task.await.unwrap();
    }
    #[test]
    fn slow_readers_are_isolated_and_stop_is_immediate() {
        let live = LiveResource::new("127.0.0.1".parse().unwrap());
        let (_slow, slow_stop) = live.subscribe(None);
        let (mut fast, fast_stop) = live.subscribe(None);
        let mut packet = [0xff; 188];
        packet[..4].copy_from_slice(&[0x47, 0x1f, 0xff, 0x10]);
        for _ in 0..17 {
            live.write(&packet).unwrap();
            fast.try_recv().unwrap();
        }
        assert!(slow_stop.is_cancelled());
        assert!(!fast_stop.is_cancelled());
        live.revoke();
        assert!(fast_stop.is_cancelled());
        assert!(live.write(&packet).is_err());
    }
}
