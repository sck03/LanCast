//! UPnP control is deliberately independent from the authenticated mirror session.
use anyhow::{Context, ensure};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::{net::IpAddr, time::Duration};
use url::Url;
const MAX_XML: usize = 512 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Service {
    pub kind: String,
    pub control: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Renderer {
    pub id: String,
    pub name: String,
    pub ip: IpAddr,
    pub transport: Service,
    pub rendering: Option<Service>,
    pub connection: Option<Service>,
}
pub fn lan_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ip.is_private() || ip.is_link_local(),
        IpAddr::V6(ip) => ip.is_unique_local() || ip.is_unicast_link_local(),
    }
}
pub fn checked_url(raw: &str, source: IpAddr) -> anyhow::Result<Url> {
    let url = Url::parse(raw)?;
    ensure!(
        matches!(url.scheme(), "http" | "https")
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none(),
        "UNSAFE_LOCATION"
    );
    ensure!(lan_ip(source), "NOT_LAN");
    let host = url
        .host_str()
        .context("NO_HOST")?
        .trim_matches(['[', ']'])
        .parse::<IpAddr>()?;
    ensure!(host == source, "LOCATION_HOST_MISMATCH");
    Ok(url)
}
fn xml(raw: &str) -> anyhow::Result<roxmltree::Document<'_>> {
    ensure!(
        raw.len() <= MAX_XML && !raw.to_ascii_uppercase().contains("<!DOCTYPE"),
        "UNSAFE_XML"
    );
    let doc = roxmltree::Document::parse_with_options(
        raw,
        roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: 8192,
            ..Default::default()
        },
    )?;
    ensure!(
        !doc.descendants()
            .any(|n| n.ancestors().take(34).count() > 33),
        "XML_TOO_DEEP"
    );
    Ok(doc)
}
pub fn parse_renderer(raw: &str, location: &str, source: IpAddr) -> anyhow::Result<Renderer> {
    let location = checked_url(location, source)?;
    let doc = xml(raw)?;
    let text = |node: roxmltree::Node<'_, '_>, name: &str| {
        node.children()
            .find(|n| n.has_tag_name(name))
            .and_then(|n| n.text())
            .unwrap_or("")
            .trim()
            .to_string()
    };
    let base = doc
        .descendants()
        .find(|n| n.has_tag_name("URLBase"))
        .and_then(|n| n.text())
        .filter(|s| !s.trim().is_empty())
        .map(|s| checked_url(s.trim(), source))
        .transpose()?
        .unwrap_or(location);
    let device = doc
        .descendants()
        .find(|n| {
            n.has_tag_name("device")
                && text(*n, "deviceType").starts_with("urn:schemas-upnp-org:device:MediaRenderer:")
        })
        .context("NOT_RENDERER")?;
    let list = device
        .children()
        .find(|n| n.has_tag_name("serviceList"))
        .context("NO_SERVICES")?;
    let mut services = Vec::new();
    for node in list.children().filter(|n| n.has_tag_name("service")) {
        let kind = text(node, "serviceType");
        let control = base.join(&text(node, "controlURL"))?;
        checked_url(control.as_str(), source)?;
        services.push(Service {
            kind,
            control: control.to_string(),
        });
    }
    let get = |suffix: &str| {
        services
            .iter()
            .find(|s| {
                s.kind
                    .starts_with(&format!("urn:schemas-upnp-org:service:{suffix}:"))
            })
            .cloned()
    };
    let id = text(device, "UDN");
    ensure!(!id.is_empty() && id.len() <= 256, "INVALID_UDN");
    Ok(Renderer {
        id,
        name: text(device, "friendlyName"),
        ip: source,
        transport: get("AVTransport").context("NO_AVTRANSPORT")?,
        rendering: get("RenderingControl"),
        connection: get("ConnectionManager"),
    })
}
pub fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
#[derive(Clone)]
pub struct Controller {
    client: reqwest::Client,
    pub device: Renderer,
}
impl Controller {
    pub fn new(device: Renderer) -> anyhow::Result<Self> {
        Ok(Self {
            client: client()?,
            device,
        })
    }
    pub async fn action(
        &self,
        service: &Service,
        action: &str,
        arguments: &[(&str, String)],
    ) -> anyhow::Result<String> {
        checked_url(&service.control, self.device.ip)?;
        ensure!(
            action.chars().all(|c| c.is_ascii_alphanumeric()),
            "INVALID_ACTION"
        );
        let args = arguments
            .iter()
            .map(|(key, value)| format!("<{key}>{}</{key}>", escape(value)))
            .collect::<String>();
        let body = format!(
            "<?xml version=\"1.0\"?><s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\"><s:Body><u:{action} xmlns:u=\"{}\">{args}</u:{action}></s:Body></s:Envelope>",
            escape(&service.kind)
        );
        let response = self
            .client
            .post(&service.control)
            .header("Content-Type", "text/xml; charset=\"utf-8\"")
            .header("SOAPACTION", format!("\"{}#{action}\"", service.kind))
            .body(body)
            .send()
            .await?;
        let status = response.status();
        let raw = bounded(response).await?;
        let doc = xml(&raw)?;
        if doc.descendants().any(|n| n.has_tag_name("Fault")) {
            anyhow::bail!(
                "DLNA_SOAP_FAULT: {}",
                doc.descendants()
                    .find(|n| n.has_tag_name("errorCode"))
                    .and_then(|n| n.text())
                    .unwrap_or("unknown")
            );
        }
        ensure!(status.is_success(), "DLNA_HTTP_ERROR");
        Ok(raw)
    }
    pub async fn load(&self, url: &str, title: &str) -> anyhow::Result<()> {
        let didl = format!(
            "<DIDL-Lite xmlns=\"urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:upnp=\"urn:schemas-upnp-org:metadata-1-0/upnp/\"><item id=\"0\" parentID=\"-1\" restricted=\"1\"><dc:title>{}</dc:title><upnp:class>object.item.videoItem</upnp:class><res protocolInfo=\"http-get:*:video/mp4:*\">{}</res></item></DIDL-Lite>",
            escape(title),
            escape(url)
        );
        self.action(
            &self.device.transport,
            "SetAVTransportURI",
            &[
                ("InstanceID", "0".into()),
                ("CurrentURI", url.into()),
                ("CurrentURIMetaData", didl),
            ],
        )
        .await?;
        Ok(())
    }
    pub async fn command(&self, action: &str, value: Option<u64>) -> anyhow::Result<String> {
        let mut args = vec![("InstanceID", "0".into())];
        let action = match action {
            "play" => {
                args.push(("Speed", "1".into()));
                "Play"
            }
            "pause" => "Pause",
            "stop" => "Stop",
            "seek" => {
                let seconds = value.context("POSITION_REQUIRED")? / 1000;
                args.push(("Unit", "REL_TIME".into()));
                args.push((
                    "Target",
                    format!(
                        "{:02}:{:02}:{:02}",
                        seconds / 3600,
                        seconds / 60 % 60,
                        seconds % 60
                    ),
                ));
                "Seek"
            }
            "state" => "GetTransportInfo",
            "position" => "GetPositionInfo",
            _ => anyhow::bail!("CAPABILITY_UNSUPPORTED"),
        };
        // Mutations are NEVER automatically retried: a timeout can mean already executed.
        match self.action(&self.device.transport, action, &args).await {
            Ok(v) => Ok(v),
            Err(_) if matches!(action, "GetTransportInfo" | "GetPositionInfo") => {
                self.action(&self.device.transport, action, &args).await
            }
            Err(e) => Err(e),
        }
    }
}
pub fn client() -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(5))
        .build()?)
}
pub async fn bounded(response: reqwest::Response) -> anyhow::Result<String> {
    ensure!(
        response.content_length().unwrap_or(0) <= MAX_XML as u64,
        "XML_TOO_LARGE"
    );
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        ensure!(bytes.len() + chunk.len() <= MAX_XML, "XML_TOO_LARGE");
        bytes.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8(bytes)?)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_ssrf_and_dtd() {
        let ip = "192.168.1.5".parse().unwrap();
        for url in [
            "http://127.0.0.1/x",
            "http://192.168.1.6/x",
            "http://example.com/x",
            "file:///etc/passwd",
            "http://user@192.168.1.5/x",
        ] {
            assert!(checked_url(url, ip).is_err());
        }
        assert!(xml("<!DOCTYPE x><x/>").is_err());
    }
    #[test]
    fn device_service_is_required() {
        let raw = "<root><device><deviceType>urn:schemas-upnp-org:device:MediaRenderer:1</deviceType><UDN>uuid:tv</UDN><friendlyName>TV</friendlyName><serviceList><service><serviceType>urn:schemas-upnp-org:service:AVTransport:1</serviceType><controlURL>/control</controlURL></service></serviceList></device></root>";
        let r = parse_renderer(
            raw,
            "http://192.168.1.5/device.xml",
            "192.168.1.5".parse().unwrap(),
        )
        .unwrap();
        assert_eq!(r.transport.control, "http://192.168.1.5/control");
        assert!(
            parse_renderer(
                &raw.replace("AVTransport", "Unknown"),
                "http://192.168.1.5/device.xml",
                r.ip
            )
            .is_err()
        );
    }
}
