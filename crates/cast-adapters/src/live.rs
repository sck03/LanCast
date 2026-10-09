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
const START_TIMEOUT: Duration = Duration::from_secs(8);
struct Reader {
    tx: mpsc::Sender<(Instant, Bytes)>,
    stop: CancellationToken,
}

struct Subscription {
    rx: mpsc::Receiver<(Instant, Bytes)>,
    stop: CancellationToken,
    gate: Option<StartGate>,
    started: Instant,
    gate_started: Instant,
    resource: Arc<LiveResource>,
}
impl Subscription {
    async fn next_chunk(&mut self) -> std::io::Result<Bytes> {
        loop {
            let wait = if self.gate.is_some() {
                START_TIMEOUT
                    .checked_sub(self.started.elapsed())
                    .filter(|remaining| !remaining.is_zero())
                    .ok_or_else(|| std::io::Error::other("STREAM_TIMEOUT"))?
            } else {
                Duration::from_secs(2)
            };
            let message = tokio::select! { biased;
                _ = self.stop.cancelled() => return Err(std::io::Error::other("STREAM_REVOKED_OR_SLOW_READER")),
                r = tokio::time::timeout(wait, self.rx.recv()) => r,
            };
            let (timestamp, data) = message
                .map_err(|_| std::io::Error::other("STREAM_TIMEOUT"))?
                .ok_or_else(|| std::io::Error::other("STREAM_CLOSED"))?;
            // timeout polls the channel before its timer; check again after an
            // executor delay so a newly ready block cannot revive expired startup.
            if self.gate.is_some() && self.started.elapsed() >= START_TIMEOUT {
                return Err(std::io::Error::other("STREAM_TIMEOUT"));
            }
            if timestamp.elapsed() > Duration::from_secs(2) {
                return Err(std::io::Error::other("SLOW_READER"));
            }
            let data = if let Some(gate) = self.gate.as_mut() {
                // Bound undecodable startup content independently of the total deadline.
                if self.gate_started.elapsed() > Duration::from_secs(2) {
                    *gate = StartGate::default();
                    self.gate_started = Instant::now();
                }
                let Some(start) = gate.push(&data).map_err(std::io::Error::other)? else {
                    continue;
                };
                self.gate = None;
                Bytes::from(start)
            } else {
                // Ingress already validated this block; keep the shared allocation
                // once this subscriber has its own decodable starting point.
                data
            };
            self.resource
                .delivered
                .fetch_add(data.len() as u64, Ordering::Release);
            return Ok(data);
        }
    }
}
impl Drop for Subscription {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

pub struct LiveResource {
    token: String,
    allowed: IpAddr,
    stop: CancellationToken,
    readers: Mutex<Vec<Reader>>,
    pulls: AtomicU64,
    ready: std::sync::atomic::AtomicBool,
    ingress: Mutex<StartGate>,
    delivered: AtomicU64,
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
            ready: std::sync::atomic::AtomicBool::new(false),
            ingress: Mutex::new(StartGate::default()),
            delivered: AtomicU64::new(0),
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
    pub fn cancellation(&self) -> CancellationToken {
        self.stop.clone()
    }
    pub fn ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }
    pub fn delivered(&self) -> u64 {
        self.delivered.load(Ordering::Acquire)
    }
    pub fn active_readers(&self) -> usize {
        self.readers
            .lock()
            .map(|r| {
                r.iter()
                    .filter(|r| !r.stop.is_cancelled() && !r.tx.is_closed())
                    .count()
            })
            .unwrap_or(0)
    }
    pub fn write(&self, data: &[u8]) -> anyhow::Result<()> {
        ensure!(!self.stop.is_cancelled(), "RESOURCE_CLOSED");
        ts::validate(data)?;
        if !self.ready()
            && self
                .ingress
                .lock()
                .map_err(|_| anyhow::anyhow!("RESOURCE_CLOSED"))?
                .push(data)?
                .is_some()
        {
            self.ready.store(true, Ordering::Release);
        }
        let mut readers = self
            .readers
            .lock()
            .map_err(|_| anyhow::anyhow!("RESOURCE_CLOSED"))?;
        ensure!(!self.stop.is_cancelled(), "RESOURCE_CLOSED");
        readers.retain(|r| !r.stop.is_cancelled() && !r.tx.is_closed());
        if readers.is_empty() {
            return Ok(());
        }
        let block = Bytes::copy_from_slice(data);
        let now = Instant::now();
        readers.retain(|r| {
            if r.tx.try_send((now, block.clone())).is_err() {
                r.stop.cancel();
                false
            } else {
                true
            }
        });
        Ok(())
    }
    fn subscribe(self: &Arc<Self>, connection: Option<CancellationToken>) -> Subscription {
        // 16 * 65424 = 1046784 bytes, below the per-reader 1 MiB budget.
        let (tx, rx) = mpsc::channel(16);
        let stop = connection.unwrap_or_else(|| self.stop.child_token());
        let mut readers = self.readers.lock().unwrap();
        readers.retain(|r| !r.tx.is_closed() && !r.stop.is_cancelled());
        if self.stop.is_cancelled() || stop.is_cancelled() {
            stop.cancel();
        } else {
            readers.push(Reader {
                tx,
                stop: stop.clone(),
            });
        }
        let now = Instant::now();
        Subscription {
            rx,
            stop,
            gate: Some(StartGate::default()),
            started: now,
            gate_started: now,
            resource: self.clone(),
        }
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
        let subscription = self.subscribe(request.extensions().get::<CancellationToken>().cloned());
        let chunks = stream::try_unfold(subscription, |mut subscription| async move {
            let data = subscription.next_chunk().await?;
            Ok::<_, std::io::Error>(Some((Frame::data(data), subscription)))
        });
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

    fn null_packet() -> [u8; 188] {
        let mut packet = [0xff; 188];
        packet[..4].copy_from_slice(&[0x47, 0x1f, 0xff, 0x10]);
        packet
    }

    #[tokio::test]
    async fn ready_subscribers_share_the_ingress_allocation() {
        let live = LiveResource::new("127.0.0.1".parse().unwrap());
        let mut first = live.subscribe(None);
        let mut second = live.subscribe(None);
        // Both readers have already passed their independent startup gate.
        first.gate = None;
        second.gate = None;
        let packet = null_packet();
        live.write(&packet).unwrap();
        let first_data = first.next_chunk().await.unwrap();
        let second_data = second.next_chunk().await.unwrap();
        assert_eq!(first_data.as_ref(), packet);
        assert_eq!(second_data.as_ref(), packet);
        assert_eq!(first_data.as_ptr(), second_data.as_ptr());
        assert_eq!(live.delivered(), (2 * packet.len()) as u64);
    }

    #[tokio::test]
    async fn startup_deadline_rejects_even_buffered_data() {
        let live = LiveResource::new("127.0.0.1".parse().unwrap());
        let mut subscription = live.subscribe(None);
        subscription.started -= Duration::from_secs(9);
        live.write(&null_packet()).unwrap();
        assert_eq!(
            subscription.next_chunk().await.unwrap_err().to_string(),
            "STREAM_TIMEOUT"
        );
        // A zero-duration timeout alone would still consume a ready channel.
        assert_eq!(subscription.rx.len(), 1);
        drop(subscription);
        assert_eq!(live.active_readers(), 0);
    }

    #[tokio::test]
    async fn stale_media_is_rejected_and_revoke_interrupts_waiting() {
        let live = LiveResource::new("127.0.0.1".parse().unwrap());
        let mut subscription = live.subscribe(None);
        subscription.gate = None;
        live.readers.lock().unwrap()[0]
            .tx
            .try_send((
                Instant::now() - Duration::from_secs(3),
                Bytes::copy_from_slice(&null_packet()),
            ))
            .unwrap();
        assert_eq!(
            subscription.next_chunk().await.unwrap_err().to_string(),
            "SLOW_READER"
        );
        let waiting = tokio::spawn(async move { subscription.next_chunk().await });
        tokio::task::yield_now().await;
        live.revoke();
        assert!(
            tokio::time::timeout(Duration::from_secs(1), waiting)
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );
        assert_eq!(live.active_readers(), 0);
    }

    #[test]
    fn dropped_and_late_subscriptions_do_not_retain_readers() {
        let live = LiveResource::new("127.0.0.1".parse().unwrap());
        let connection = CancellationToken::new();
        let subscription = live.subscribe(Some(connection.clone()));
        drop(subscription);
        assert!(connection.is_cancelled());
        assert!(!live.stop.is_cancelled());
        live.write(&null_packet()).unwrap();
        assert!(live.readers.lock().unwrap().is_empty());
        live.revoke();
        let late = live.subscribe(Some(CancellationToken::new()));
        assert!(late.stop.is_cancelled());
        assert!(late.rx.is_closed());
        assert!(live.readers.lock().unwrap().is_empty());
    }

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
        let slow = live.subscribe(None);
        let mut fast = live.subscribe(None);
        let packet = null_packet();
        for _ in 0..17 {
            live.write(&packet).unwrap();
            fast.rx.try_recv().unwrap();
        }
        assert!(slow.stop.is_cancelled());
        assert!(!fast.stop.is_cancelled());
        live.revoke();
        assert!(fast.stop.is_cancelled());
        assert!(live.write(&packet).is_err());
    }
}
