//! UPnP control is deliberately independent from the authenticated mirror session.
mod description;
mod soap;
pub use description::parse_renderer;
pub use soap::CommandResult;

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
    #[serde(default)]
    pub signature: String,
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
    ensure!(raw.len() <= MAX_XML, "UNSAFE_XML");
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
pub fn escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}
#[derive(Clone)]
pub struct Controller {
    client: reqwest::Client,
    pub device: Renderer,
}
#[derive(Clone, Copy)]
pub enum MediaKind {
    FileMp4,
    LiveTs,
}
impl Controller {
    pub async fn state(&self) -> anyhow::Result<String> {
        self.query("GetTransportInfo", soap::state).await
    }
    pub async fn current_uri(&self) -> anyhow::Result<String> {
        self.action(
            "GetMediaInfo",
            &[("InstanceID", "0".into())],
            soap::current_uri,
        )
        .await
    }
    pub fn new(device: Renderer) -> anyhow::Result<Self> {
        Ok(Self {
            client: client()?,
            device,
        })
    }
    async fn action<T>(
        &self,
        action: &str,
        arguments: &[(&str, String)],
        decode: fn(roxmltree::Node<'_, '_>) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let service = &self.device.transport;
        checked_url(&service.control, self.device.ip)?;
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
        let status = response.status().as_u16();
        let raw = bounded(response).await?;
        soap::decode(&raw, status, &service.kind, action, decode)
    }
    async fn query<T>(
        &self,
        action: &str,
        decode: fn(roxmltree::Node<'_, '_>) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let args = [("InstanceID", "0".into())];
        match self.action(action, &args, decode).await {
            Ok(result) => Ok(result),
            Err(_) => self.action(action, &args, decode).await,
        }
    }
    pub async fn load_media(&self, url: &str, title: &str, kind: MediaKind) -> anyhow::Result<()> {
        let mime = match kind {
            MediaKind::FileMp4 => "video/mp4",
            MediaKind::LiveTs => "video/mpeg",
        };
        let didl = format!(
            "<DIDL-Lite xmlns=\"urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:upnp=\"urn:schemas-upnp-org:metadata-1-0/upnp/\"><item id=\"0\" parentID=\"-1\" restricted=\"1\"><dc:title>{}</dc:title><upnp:class>object.item.videoItem</upnp:class><res protocolInfo=\"http-get:*:{mime}:*\">{}</res></item></DIDL-Lite>",
            escape(title),
            escape(url)
        );
        self.action(
            "SetAVTransportURI",
            &[
                ("InstanceID", "0".into()),
                ("CurrentURI", url.into()),
                ("CurrentURIMetaData", didl),
            ],
            |_| Ok(()),
        )
        .await?;
        Ok(())
    }
    pub async fn command(&self, action: &str, value: Option<u64>) -> anyhow::Result<CommandResult> {
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
            "state" => {
                return Ok(CommandResult::State {
                    transport_state: self.state().await?,
                });
            }
            "position" => return self.query("GetPositionInfo", soap::position).await,
            _ => anyhow::bail!("CAPABILITY_UNSUPPORTED"),
        };
        // Mutations are NEVER automatically retried: a timeout can mean already executed.
        self.action(action, &args, |_| Ok(CommandResult::Accepted))
            .await
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
    fn unrelated_services_cannot_hide_a_playable_renderer() {
        let raw = "<root><device><deviceType>urn:schemas-upnp-org:device:MediaRenderer:1</deviceType><UDN>uuid:tv</UDN><serviceList><service><serviceType>urn:vendor:service:Diagnostics:1</serviceType><controlURL>http://example.com/diagnostics</controlURL></service><service><serviceType>urn:schemas-upnp-org:service:AVTransport:1</serviceType><controlURL>/control</controlURL></service></serviceList></device></root>";
        let renderer = parse_renderer(
            raw,
            "http://192.168.1.5/device.xml",
            "192.168.1.5".parse().unwrap(),
        )
        .unwrap();
        assert_eq!(renderer.transport.control, "http://192.168.1.5/control");
    }

    #[test]
    fn missing_control_endpoint_is_not_the_description_endpoint() {
        for control in ["", " ", "\n\t"] {
            let raw = format!(
                "<root><device><deviceType>urn:schemas-upnp-org:device:MediaRenderer:1</deviceType><UDN>uuid:tv</UDN><serviceList><service><serviceType>urn:schemas-upnp-org:service:AVTransport:1</serviceType><controlURL>{control}</controlURL></service></serviceList></device></root>"
            );
            assert!(
                parse_renderer(
                    &raw,
                    "http://192.168.1.5/device.xml",
                    "192.168.1.5".parse().unwrap()
                )
                .is_err()
            );
        }
    }

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
        assert!(xml("<!DOCTYPE x [<!ENTITY a 'value'>]><x>&a;</x>").is_err());
        assert!(xml("<!DOCTYPE x SYSTEM 'http://192.168.1.5/dtd'><x/>").is_err());
        // Harmless comments are not declarations; the XML parser owns DTD rejection.
        assert!(xml("<!-- <!DOCTYPE x> --><x/>").is_ok());
        assert!(xml(&format!("<x>{}</x>", "x".repeat(MAX_XML))).is_err());
        assert!(xml(&format!("<x>{}</x>", "<y/>".repeat(8192))).is_err());
        assert!(xml(&format!("{}{}", "<x>".repeat(34), "</x>".repeat(34))).is_err());
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
        for endpoint in ["http://192.168.1.6/control", "//example.com/control"] {
            assert!(
                parse_renderer(
                    &raw.replace(">/control<", &format!(">{endpoint}<")),
                    "http://192.168.1.5/device.xml",
                    r.ip
                )
                .is_err()
            );
        }
        let relative = raw.replace(">/control<", ">control<");
        let based = relative.replace(
            "<root>",
            "<root><URLBase>http://192.168.1.5/upnp/</URLBase>",
        );
        assert_eq!(
            parse_renderer(&based, "http://192.168.1.5/device.xml", r.ip)
                .unwrap()
                .transport
                .control,
            "http://192.168.1.5/upnp/control"
        );
        let nested = relative.replace("<device>", "<device><URLBase>http://192.168.1.6/</URLBase>");
        assert_eq!(
            parse_renderer(&nested, "http://192.168.1.5/upnp/device.xml", r.ip)
                .unwrap()
                .transport
                .control,
            "http://192.168.1.5/upnp/control"
        );
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
