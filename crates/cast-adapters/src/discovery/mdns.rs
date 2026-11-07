//! LanCast advertisements and asynchronous discovery. Hints never establish trust.
use super::{SCAN_WINDOW, results::Results};
use crate::auth;
use futures_util::{Stream, StreamExt};
use std::net::{IpAddr, Ipv4Addr};
use tokio::time::Instant;

/// Shutdown also runs after browse/register errors or cancellation of a search.
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
    Ok(collect_lancast(events.into_stream(), Instant::now() + SCAN_WINDOW).await)
}

async fn collect_lancast(
    events: impl Stream<Item = mdns_sd::ServiceEvent>,
    deadline: Instant,
) -> Vec<serde_json::Value> {
    let mut found = Results::new();
    tokio::pin!(events);
    while !found.is_full() && Instant::now() < deadline {
        let Ok(Some(message)) = tokio::time::timeout_at(deadline, events.next()).await else {
            break;
        };
        // A ready multicast stream must still allow cancellation and other work.
        tokio::task::consume_budget().await;
        if Instant::now() >= deadline {
            break;
        }
        if let mdns_sd::ServiceEvent::ServiceResolved(info) = message
            && let Some(device) = resolved_device(&info)
        {
            let id = info
                .get_property_val_str("deviceId")
                .unwrap_or(info.get_fullname());
            found.insert(id.to_owned(), device);
        }
    }
    found.into_devices()
}

fn resolved_device(info: &mdns_sd::ResolvedService) -> Option<serde_json::Value> {
    let address = info
        .get_property_val_str("address")?
        .parse::<Ipv4Addr>()
        .ok()?;
    if !info
        .get_addresses()
        .iter()
        .any(|ip| ip.to_ip_addr() == IpAddr::V4(address))
    {
        return None;
    }
    let fingerprint = advertised_fingerprint(
        info.get_property_val_str("fingerprint"),
        info.get_property_val_str("pairingVersion"),
    )?;
    let id = info
        .get_property_val_str("deviceId")
        .unwrap_or(info.get_fullname());
    Some(
        serde_json::json!({"id":id,"name":info.get_fullname(),"addresses":[address],"port":info.get_port(),"fingerprint":fingerprint,"kind":"lancast_receiver","verification":"unverified"}),
    )
}

fn advertised_fingerprint(value: Option<&str>, version: Option<&str>) -> Option<String> {
    if version.and_then(|value| value.parse::<u64>().ok()) != Some(auth::PAIRING_VERSION) {
        return None;
    }
    value
        .filter(|v| v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(str::to_ascii_lowercase)
}

#[cfg(test)]
mod tests {
    use super::super::results::MAX_RESULT_BYTES;
    use super::*;
    use crate::events::event;
    use futures_util::{FutureExt, stream};
    use std::time::Duration;
    use tokio::sync::oneshot;
    use tokio_util::sync::CancellationToken;

    fn service(name: &str, id: &str, address: &str) -> mdns_sd::ResolvedService {
        let pin = "A".repeat(64);
        mdns_sd::ServiceInfo::new(
            "_lancast._tcp.local.",
            name,
            "receiver.local.",
            "192.168.1.2",
            9443,
            [
                ("deviceId", id),
                ("fingerprint", pin.as_str()),
                ("pairingVersion", "2"),
                ("address", address),
            ]
            .as_slice(),
        )
        .unwrap()
        .as_resolved_service()
    }

    #[tokio::test]
    async fn mdns_discovery_budget_prevents_host_shutdown() {
        let messages = (0..64).map(|n| {
            // Each DNS label fits 63 bytes and each TXT property fits 255 bytes.
            // JSON escaping can still expand the combined result past 128 KiB.
            let name = format!("{n:02}{}", "\u{1}".repeat(61));
            let id = format!("{n:02}{}", "\u{1}".repeat(244));
            mdns_sd::ServiceEvent::ServiceResolved(Box::new(service(&name, &id, "192.168.1.2")))
        });
        let found = collect_lancast(stream::iter(messages), Instant::now() + SCAN_WINDOW).await;
        assert!(!found.is_empty() && found.len() < 64);
        let stop = CancellationToken::new();
        let (events, inbox) = crate::events::channel(2, stop.clone());
        event(
            &events,
            "devices",
            serde_json::json!({"devices":found,"scanGeneration":u64::MAX}),
        );
        let delivered: serde_json::Value =
            serde_json::from_str(&inbox.try_recv().unwrap()).unwrap();
        assert!(!stop.is_cancelled(), "unexpected event: {delivered}");
        assert_eq!(delivered["type"], "devices");
        assert!(
            serde_json::to_vec(&delivered["body"]["devices"])
                .unwrap()
                .len()
                <= MAX_RESULT_BYTES
        );
    }

    #[test]
    fn discovery_requires_a_resolved_address_current_pairing_and_full_pin() {
        let valid = service("Living room", "tv", "192.168.1.2");
        let device = resolved_device(&valid).unwrap();
        assert_eq!(device["addresses"], serde_json::json!(["192.168.1.2"]));
        assert_eq!(device["fingerprint"], "a".repeat(64));
        assert_eq!(device["verification"], "unverified");
        for address in ["192.168.1.3", "::1", "", "invalid"] {
            assert!(resolved_device(&service("TV", "tv", address)).is_none());
        }
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
    }

    #[tokio::test(start_paused = true)]
    async fn late_ready_mdns_result_cannot_extend_the_deadline() {
        let (release, pending) = oneshot::channel();
        let messages = stream::once(async {
            pending.await.unwrap();
            mdns_sd::ServiceEvent::ServiceResolved(Box::new(service("TV", "tv", "192.168.1.2")))
        });
        let mut collection = Box::pin(collect_lancast(
            messages,
            Instant::now() + Duration::from_secs(4),
        ));
        assert!(collection.as_mut().now_or_never().is_none());
        tokio::time::advance(Duration::from_secs(5)).await;
        release.send(()).unwrap();
        assert!(collection.await.is_empty());
    }
}
