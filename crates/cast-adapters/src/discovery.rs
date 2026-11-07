mod mdns;
mod results;
mod tasks;
pub use mdns::{Advertisement, scan_lancast};
use results::{MAX_DEVICES, Results};
pub(crate) use tasks::{DlnaDiscovery, ScanTask};

use crate::dlna::{self, Renderer};
use anyhow::ensure;
use futures_util::{Stream, StreamExt, stream};
use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr},
    time::Duration,
};
use tokio::time::Instant;

const SCAN_WINDOW: Duration = Duration::from_secs(4);

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
    let mut found = Results::new();
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
    found.into_devices()
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
    use super::results::MAX_RESULT_BYTES;
    use super::*;
    use crate::events::event;
    use tokio::sync::oneshot;
    use tokio_util::sync::CancellationToken;

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

    #[tokio::test]
    async fn oversized_descriptions_do_not_overflow_the_host_mailbox() {
        let mut oversized = renderer();
        oversized.id = "oversized".into();
        oversized.name = "x".repeat(128 * 1024);
        let jobs = stream::iter([Ok(oversized), Ok(renderer())]);
        let found = collect_renderers(jobs, Instant::now() + SCAN_WINDOW).await;
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "tv");
        let stop = CancellationToken::new();
        let (events, inbox) = crate::events::channel(2, stop.clone());
        event(
            &events,
            "dlna.devices",
            serde_json::json!({"devices":found,"scanGeneration":1}),
        );
        assert!(!stop.is_cancelled());
        assert!(inbox.try_recv().unwrap().contains("dlna.devices"));
    }

    #[tokio::test]
    async fn discovery_budget_counts_json_escaping_and_all_devices() {
        let jobs = stream::iter((0..64).map(|n| {
            let mut device = renderer();
            device.id = n.to_string();
            device.name = "\u{1}电视\\\"".repeat(1024);
            Ok(device)
        }));
        let found = collect_renderers(jobs, Instant::now() + SCAN_WINDOW).await;
        assert!(!found.is_empty() && found.len() < 64);
        assert!(serde_json::to_vec(&found).unwrap().len() <= MAX_RESULT_BYTES);
        let stop = CancellationToken::new();
        let (events, _) = crate::events::channel(2, stop.clone());
        event(
            &events,
            "dlna.devices",
            serde_json::json!({"devices":found}),
        );
        assert!(!stop.is_cancelled());
    }

    #[tokio::test]
    async fn replacement_preserves_or_releases_its_previous_byte_budget() {
        let device = |id: &str, kib: usize| {
            let mut device = renderer();
            device.id = id.into();
            device.name = "x".repeat(kib * 1024);
            Ok(device)
        };
        let jobs = stream::iter([device("a", 45), device("b", 45), device("a", 60)]);
        let found = collect_renderers(jobs, Instant::now() + SCAN_WINDOW).await;
        assert_eq!(found.len(), 2);
        assert_eq!(
            found.iter().find(|d| d.id == "a").unwrap().name.len(),
            45 * 1024
        );

        let jobs = stream::iter([
            device("a", 45),
            device("b", 45),
            device("a", 1),
            device("c", 45),
        ]);
        let found = collect_renderers(jobs, Instant::now() + SCAN_WINDOW).await;
        assert_eq!(found.len(), 3);
        assert_eq!(found.iter().find(|d| d.id == "a").unwrap().name.len(), 1024);
        assert!(serde_json::to_vec(&found).unwrap().len() <= MAX_RESULT_BYTES);
    }

    #[tokio::test]
    async fn collection_enforces_the_device_count_independently_of_the_source() {
        let jobs = stream::iter((0..MAX_DEVICES + 10).map(|n| {
            let mut device = renderer();
            device.id = n.to_string();
            Ok(device)
        }));
        let found = collect_renderers(jobs, Instant::now() + SCAN_WINDOW).await;
        assert_eq!(found.len(), MAX_DEVICES);
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
}
