use crate::{
    auth,
    dlna::{self, Renderer},
    events::{Events, event},
};
use anyhow::ensure;
use futures_util::{Stream, StreamExt, stream};
use std::{
    collections::HashMap,
    future::Future,
    net::{IpAddr, Ipv4Addr},
    time::Duration,
};
use tokio::{task::JoinHandle, time::Instant};

const SCAN_WINDOW: Duration = Duration::from_secs(4);
const MAX_DEVICES: usize = 64;

/// Owns the one mDNS search for a host; replacing or stopping it joins cancellation.
#[derive(Default)]
pub(crate) struct ScanTask(Option<JoinHandle<()>>);
impl ScanTask {
    pub async fn start(&mut self, events: Events, generation: serde_json::Value) {
        self.replace(async move {
            let body = match scan_lancast().await {
                Ok(devices) => serde_json::json!({"devices":devices,"scanGeneration":generation}),
                Err(_) => serde_json::json!({"devices":[],"scanGeneration":generation,"error":"DISCOVERY_FAILED"}),
            };
            event(&events, "devices", body);
        })
        .await;
    }
    async fn replace(&mut self, work: impl Future<Output = ()> + Send + 'static) {
        self.stop().await;
        self.0 = Some(tokio::spawn(work));
    }
    pub async fn stop(&mut self) {
        if let Some(task) = self.0.take() {
            task.abort();
            let _ = task.await;
        }
    }
}
impl Drop for ScanTask {
    fn drop(&mut self) {
        if let Some(task) = &self.0 {
            task.abort();
        }
    }
}

/// Shutdown also runs after browse/register errors or cancellation of an async search.
struct MdnsDaemon(mdns_sd::ServiceDaemon);
impl MdnsDaemon {
    fn new() -> anyhow::Result<Self> {
        Ok(Self(mdns_sd::ServiceDaemon::new()?))
    }
}
impl Drop for MdnsDaemon {
    fn drop(&mut self) {
        let _ = self.0.shutdown();
    }
}

pub struct Advertisement {
    daemon: MdnsDaemon,
    name: String,
}
impl Advertisement {
    pub fn start(
        id: &str,
        name: &str,
        ip: Ipv4Addr,
        port: u16,
        variant: &str,
        fingerprint: &str,
    ) -> anyhow::Result<Self> {
        let daemon = MdnsDaemon::new()?;
        let pairing_version = auth::PAIRING_VERSION.to_string();
        let bound_address = ip.to_string();
        let info = mdns_sd::ServiceInfo::new(
            "_lancast._tcp.local.",
            name,
            &format!("lancast-{id}.local."),
            IpAddr::V4(ip),
            port,
            [
                ("version", "1"),
                ("deviceId", id),
                ("variant", variant),
                ("fingerprint", fingerprint),
                ("pairingVersion", pairing_version.as_str()),
                ("address", bound_address.as_str()),
            ]
            .as_slice(),
        )?;
        let name = info.get_fullname().to_string();
        daemon.0.register(info)?;
        Ok(Self { daemon, name })
    }
}
impl Drop for Advertisement {
    fn drop(&mut self) {
        let _ = self.daemon.0.unregister(&self.name);
    }
}
pub async fn scan_lancast() -> anyhow::Result<Vec<serde_json::Value>> {
    let daemon = MdnsDaemon::new()?;
    let events = daemon.0.browse("_lancast._tcp.local.")?;
    let until = Instant::now() + SCAN_WINDOW;
    let mut found = HashMap::new();
    while found.len() < MAX_DEVICES && Instant::now() < until {
        let Ok(Ok(message)) = tokio::time::timeout_at(until, events.recv_async()).await else {
            break;
        };
        // flume can remain ready under a multicast burst; let abort/stop run too.
        tokio::task::consume_budget().await;
        if Instant::now() >= until {
            break;
        }
        if let mdns_sd::ServiceEvent::ServiceResolved(info) = message {
            let id = info
                .get_property_val_str("deviceId")
                .unwrap_or(info.get_fullname())
                .to_string();
            // Discovery is untrusted; TLS identity must still be verified by the user.
            let addresses: Vec<String> =
                info.get_addresses().iter().map(|a| a.to_string()).collect();
            let Some(address) =
                advertised_address(info.get_property_val_str("address"), &addresses)
            else {
                continue;
            };
            let addresses = vec![address];
            let Some(fingerprint) = advertised_fingerprint(
                info.get_property_val_str("fingerprint"),
                info.get_property_val_str("pairingVersion"),
            ) else {
                continue;
            };
            found.insert(id.clone(),serde_json::json!({"id":id,"name":info.get_fullname(),"addresses":addresses,"port":info.get_port(),"fingerprint":fingerprint,"kind":"lancast_receiver","verification":"unverified"}));
        }
    }
    Ok(found.into_values().collect())
}
// Public discovery hints are never a trust decision. The sender must compare the
// full value with the receiver before passing it to the pinned TLS connection.
fn advertised_fingerprint(value: Option<&str>, version: Option<&str>) -> Option<String> {
    if version.and_then(|value| value.parse::<u64>().ok()) != Some(auth::PAIRING_VERSION) {
        return None;
    }
    value
        .filter(|v| v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(str::to_ascii_lowercase)
}
fn advertised_address(value: Option<&str>, resolved: &[String]) -> Option<String> {
    value
        .filter(|value| {
            value.parse::<Ipv4Addr>().is_ok() && resolved.iter().any(|ip| ip.as_str() == *value)
        })
        .map(str::to_owned)
}

pub async fn scan_dlna(interface: Ipv4Addr) -> anyhow::Result<Vec<Renderer>> {
    ensure!(dlna::lan_ip(IpAddr::V4(interface)), "SELECT_LAN_INTERFACE");
    let socket = tokio::net::UdpSocket::bind((interface, 0)).await?;
    socket.set_multicast_ttl_v4(2)?;
    let request=b"M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: 2\r\nST: urn:schemas-upnp-org:device:MediaRenderer:1\r\n\r\n";
    socket.send_to(request, "239.255.255.250:1900").await?;
    let end = Instant::now() + SCAN_WINDOW;
    let mut locations = HashMap::new();
    let mut buffer = [0; 8192];
    while locations.len() < MAX_DEVICES && Instant::now() < end {
        let Ok(Ok((size, source))) =
            tokio::time::timeout_at(end, socket.recv_from(&mut buffer)).await
        else {
            break;
        };
        let Ok(text) = std::str::from_utf8(&buffer[..size]) else {
            continue;
        };
        if let Ok(url) = ssdp_location(text, source.ip()) {
            locations.insert(url, source.ip());
        }
    }
    let client = dlna::client()?;
    // Bounded concurrency keeps one malicious/slow device from delaying every result.
    let jobs = stream::iter(locations)
        .map(|(url, ip)| {
            let client = client.clone();
            async move {
                let response = client.get(&url).send().await?.error_for_status()?;
                let raw = dlna::bounded(response).await?;
                dlna::parse_renderer(&raw, &url, ip)
            }
        })
        .buffer_unordered(8);
    Ok(collect_renderers(jobs, end + SCAN_WINDOW).await)
}

async fn collect_renderers(
    jobs: impl Stream<Item = anyhow::Result<Renderer>>,
    deadline: Instant,
) -> Vec<Renderer> {
    let mut found = HashMap::new();
    tokio::pin!(jobs);
    // One deadline covers every batch and drops unfinished requests on return.
    while Instant::now() < deadline {
        let Ok(Some(result)) = tokio::time::timeout_at(deadline, jobs.next()).await else {
            break;
        };
        if Instant::now() >= deadline {
            break;
        }
        if let Ok(renderer) = result {
            found.insert(renderer.id.clone(), renderer);
        }
    }
    found.into_values().collect()
}
pub fn ssdp_location(text: &str, source: IpAddr) -> anyhow::Result<String> {
    ensure!(
        text.len() <= 8192 && text.starts_with("HTTP/1.1 200"),
        "INVALID_SSDP"
    );
    let mut location = None;
    for line in text.split("\r\n").skip(1) {
        if let Some((key, value)) = line.split_once(':')
            && key.eq_ignore_ascii_case("location")
        {
            ensure!(location.is_none(), "DUPLICATE_LOCATION");
            location = Some(value.trim());
        }
    }
    Ok(dlna::checked_url(
        location.ok_or_else(|| anyhow::anyhow!("NO_LOCATION"))?,
        source,
    )?
    .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::oneshot;
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn replacement_joins_old_scan_and_stop_is_idempotent() {
        let mut scan = ScanTask::default();
        let mut previous: Option<CancellationToken> = None;
        for _ in 0..8 {
            let finished = CancellationToken::new();
            let guard = finished.clone().drop_guard();
            let (started, ready) = oneshot::channel();
            let old = previous.clone();
            scan.replace(async move {
                let _guard = guard;
                assert!(old.is_none_or(|old| old.is_cancelled()));
                started.send(()).unwrap();
                std::future::pending::<()>().await;
            })
            .await;
            ready.await.unwrap();
            previous = Some(finished);
        }
        scan.stop().await;
        assert!(previous.unwrap().is_cancelled());
        scan.stop().await;
    }

    #[tokio::test]
    async fn dropping_scan_owner_cancels_pending_work() {
        let mut scan = ScanTask::default();
        let finished = CancellationToken::new();
        let guard = finished.clone().drop_guard();
        let (started, ready) = oneshot::channel();
        scan.replace(async move {
            let _guard = guard;
            started.send(()).unwrap();
            std::future::pending::<()>().await;
        })
        .await;
        ready.await.unwrap();
        drop(scan);
        tokio::time::timeout(Duration::from_secs(1), finished.cancelled())
            .await
            .unwrap();
    }

    fn renderer() -> Renderer {
        Renderer {
            id: "tv".into(),
            name: "Living room".into(),
            ip: "192.168.1.2".parse().unwrap(),
            transport: dlna::Service {
                kind: "AVTransport:1".into(),
                control: "http://192.168.1.2/control".into(),
            },
            signature: String::new(),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn description_deadline_keeps_results_and_drops_pending_requests() {
        let finished = CancellationToken::new();
        let guard = finished.clone().drop_guard();
        let (started, pending_started) = oneshot::channel();
        let jobs = stream::iter([Ok(renderer()), Err(anyhow::anyhow!("DEVICE_OFFLINE"))]).chain(
            stream::once(async move {
                let _guard = guard;
                started.send(()).unwrap();
                std::future::pending::<anyhow::Result<Renderer>>().await
            }),
        );
        let found = tokio::time::timeout(
            Duration::from_secs(1),
            collect_renderers(jobs, Instant::now() + Duration::from_millis(100)),
        )
        .await
        .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "tv");
        pending_started.await.unwrap();
        assert!(finished.is_cancelled());
    }

    #[tokio::test]
    async fn expired_description_deadline_does_not_start_more_requests() {
        let started = std::sync::atomic::AtomicBool::new(false);
        let jobs = stream::once(async {
            started.store(true, std::sync::atomic::Ordering::Relaxed);
            Err(anyhow::anyhow!("SHOULD_NOT_START"))
        });
        assert!(collect_renderers(jobs, Instant::now()).await.is_empty());
        assert!(!started.load(std::sync::atomic::Ordering::Relaxed));
    }

    #[tokio::test(start_paused = true)]
    async fn ready_description_cannot_revive_an_expired_deadline() {
        use futures_util::FutureExt;
        let (release, pending) = oneshot::channel();
        let jobs = stream::once(async {
            pending.await.unwrap();
            Ok(renderer())
        });
        let mut collection = Box::pin(collect_renderers(
            jobs,
            Instant::now() + Duration::from_millis(20),
        ));
        // Enter the pending description before advancing the injected clock.
        // timeout_at polls its now-ready work before its already-expired timer,
        // so this exercises collect_renderers' post-completion deadline check.
        assert!(collection.as_mut().now_or_never().is_none());
        tokio::time::advance(Duration::from_millis(60)).await;
        release.send(()).unwrap();
        let found = collection.await;
        assert!(found.is_empty());
    }

    #[test]
    fn discovery_requires_current_pairing_and_full_pin() {
        let resolved = vec!["192.168.1.2".to_owned(), "10.0.0.2".to_owned()];
        assert_eq!(
            advertised_address(Some("10.0.0.2"), &resolved),
            Some("10.0.0.2".into())
        );
        assert_eq!(advertised_address(Some("10.0.0.3"), &resolved), None);
        assert_eq!(advertised_address(None, &resolved), None);
        assert_eq!(advertised_fingerprint(None, Some("2")), None);
        assert_eq!(advertised_fingerprint(Some("abc"), Some("2")), None);
        assert_eq!(
            advertised_fingerprint(Some(&"g".repeat(64)), Some("2")),
            None
        );
        assert_eq!(advertised_fingerprint(Some(&"a".repeat(64)), None), None);
        assert_eq!(
            advertised_fingerprint(Some(&"a".repeat(64)), Some("1")),
            None
        );
        assert_eq!(
            advertised_fingerprint(Some(&"A".repeat(64)), Some("2")),
            Some("a".repeat(64))
        );
    }
}
