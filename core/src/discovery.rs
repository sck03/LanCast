use crate::dlna::{self, Renderer};
use anyhow::ensure;
use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr},
    time::Duration,
};
pub struct Advertisement {
    daemon: mdns_sd::ServiceDaemon,
    name: String,
}
impl Advertisement {
    pub fn start(
        id: &str,
        name: &str,
        ip: Ipv4Addr,
        port: u16,
        variant: &str,
    ) -> anyhow::Result<Self> {
        let daemon = mdns_sd::ServiceDaemon::new()?;
        let info = mdns_sd::ServiceInfo::new(
            "_lancast._tcp.local.",
            name,
            &format!("lancast-{id}.local."),
            IpAddr::V4(ip),
            port,
            [("version", "1"), ("deviceId", id), ("variant", variant)].as_slice(),
        )?;
        let name = info.get_fullname().to_string();
        daemon.register(info)?;
        Ok(Self { daemon, name })
    }
}
impl Drop for Advertisement {
    fn drop(&mut self) {
        let _ = self.daemon.unregister(&self.name);
        let _ = self.daemon.shutdown();
    }
}
pub fn scan_lancast() -> anyhow::Result<Vec<serde_json::Value>> {
    let daemon = mdns_sd::ServiceDaemon::new()?;
    let events = daemon.browse("_lancast._tcp.local.")?;
    let until = std::time::Instant::now() + Duration::from_secs(4);
    let mut found = HashMap::new();
    while std::time::Instant::now() < until {
        if let Ok(mdns_sd::ServiceEvent::ServiceResolved(info)) =
            events.recv_timeout(Duration::from_millis(250))
        {
            let id = info
                .get_property_val_str("deviceId")
                .unwrap_or(info.get_fullname())
                .to_string();
            // Discovery is untrusted; TLS identity must still be verified by the user.
            let addresses: Vec<String> =
                info.get_addresses().iter().map(|a| a.to_string()).collect();
            found.insert(id.clone(),serde_json::json!({"id":id,"name":info.get_fullname(),"addresses":addresses,"port":info.get_port(),"kind":"lancast_receiver","verification":"unverified"}));
        }
    }
    let _ = daemon.stop_browse("_lancast._tcp.local.");
    let _ = daemon.shutdown();
    Ok(found.into_values().collect())
}
pub async fn scan_dlna(interface: Ipv4Addr) -> anyhow::Result<Vec<Renderer>> {
    ensure!(dlna::lan_ip(IpAddr::V4(interface)), "SELECT_LAN_INTERFACE");
    let socket = tokio::net::UdpSocket::bind((interface, 0)).await?;
    socket.set_multicast_ttl_v4(2)?;
    let request=b"M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: 2\r\nST: urn:schemas-upnp-org:device:MediaRenderer:1\r\n\r\n";
    socket.send_to(request, "239.255.255.250:1900").await?;
    let end = tokio::time::Instant::now() + Duration::from_secs(4);
    let mut locations = HashMap::new();
    let mut buffer = [0; 8192];
    while let Ok(Ok((size, source))) =
        tokio::time::timeout_at(end, socket.recv_from(&mut buffer)).await
    {
        if locations.len() >= 64 {
            break;
        }
        let Ok(text) = std::str::from_utf8(&buffer[..size]) else {
            continue;
        };
        if let Ok(url) = ssdp_location(text, source.ip()) {
            locations.insert(url, source.ip());
        }
    }
    let client = dlna::client()?;
    let mut found = HashMap::new();
    // Bounded concurrency keeps one malicious/slow device from delaying every result.
    use futures_util::{StreamExt, stream};
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
    tokio::pin!(jobs);
    while let Some(Ok(renderer)) = jobs.next().await {
        found.insert(renderer.id.clone(), renderer);
    }
    Ok(found.into_values().collect())
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
