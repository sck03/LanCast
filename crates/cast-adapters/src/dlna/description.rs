//! Device descriptions are parsed without network I/O or host event ownership.
use super::{Renderer, Service, checked_url, xml};
use anyhow::{Context, ensure};
use sha2::{Digest, Sha256};
use std::net::IpAddr;

fn transport_version(kind: &str) -> Option<u32> {
    let version = kind
        .strip_prefix("urn:schemas-upnp-org:service:AVTransport:")?
        .parse::<u32>()
        .ok()?;
    (1..=3).contains(&version).then_some(version)
}

fn text<'a>(node: roxmltree::Node<'a, '_>, name: &str) -> &'a str {
    node.children()
        .find(|n| n.has_tag_name(name))
        .and_then(|n| n.text())
        .unwrap_or("")
        .trim()
}

pub fn parse_renderer(raw: &str, location: &str, source: IpAddr) -> anyhow::Result<Renderer> {
    let location = checked_url(location, source)?;
    let doc = xml(raw)?;
    let base = match text(doc.root_element(), "URLBase") {
        "" => location,
        value => checked_url(value, source)?,
    };
    let device = doc
        .descendants()
        .find(|n| {
            n.has_tag_name("device")
                && text(*n, "deviceType").starts_with("urn:schemas-upnp-org:device:MediaRenderer:")
        })
        .context("NOT_RENDERER")?;
    let id = text(device, "UDN");
    ensure!(!id.is_empty() && id.len() <= 256, "INVALID_UDN");
    let list = device
        .children()
        .find(|n| n.has_tag_name("serviceList"))
        .context("NO_SERVICES")?;
    // Only AVTransport is used. Vendor services must not allocate URLs or
    // prevent discovery merely because their endpoints are unsupported here.
    let transport = list
        .children()
        .filter(|n| n.has_tag_name("service"))
        .filter_map(|n| transport_version(text(n, "serviceType")).map(|version| (version, n)))
        .max_by_key(|(version, _)| *version)
        .map(|(_, node)| node)
        .context("NO_AVTRANSPORT")?;
    let endpoint = text(transport, "controlURL");
    ensure!(!endpoint.is_empty(), "NO_CONTROL_URL");
    let control = base.join(endpoint)?;
    checked_url(control.as_str(), source)?;
    Ok(Renderer {
        id: id.into(),
        name: text(device, "friendlyName").into(),
        ip: source,
        transport: Service {
            kind: text(transport, "serviceType").into(),
            control: control.into(),
        },
        signature: hex::encode(Sha256::digest(format!(
            "{}|{}|{}|{}|{}",
            id,
            text(device, "manufacturer"),
            text(device, "modelName"),
            text(device, "modelNumber"),
            text(device, "softwareVersion")
        ))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selects_highest_supported_transport_without_malformed_or_unknown_versions() {
        let service = |version: &str| {
            format!(
                "<service><serviceType>urn:schemas-upnp-org:service:AVTransport:{version}</serviceType><controlURL>/v{version}</controlURL></service>"
            )
        };
        let raw = format!(
            "<root><device><deviceType>urn:schemas-upnp-org:device:MediaRenderer:3</deviceType><UDN>tv</UDN><serviceList>{}{}{}{}</serviceList></device></root>",
            service("1"),
            service("bad"),
            service("3"),
            service("99")
        );
        let renderer = parse_renderer(
            &raw,
            "http://192.168.1.2/device.xml",
            "192.168.1.2".parse().unwrap(),
        )
        .unwrap();
        assert_eq!(
            renderer.transport.kind,
            "urn:schemas-upnp-org:service:AVTransport:3"
        );
        assert_eq!(renderer.transport.control, "http://192.168.1.2/v3");
    }
}
