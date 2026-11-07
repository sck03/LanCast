//! Decode one SOAP reply into bounded values; XML never crosses the host boundary.
use anyhow::{Context, ensure};
use roxmltree::Node;
use serde::Serialize;

const ENVELOPE: &str = "http://schemas.xmlsoap.org/soap/envelope/";

#[derive(Debug, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum CommandResult {
    Accepted,
    State {
        transport_state: String,
    },
    Position {
        relative_time: Option<String>,
        track_duration: Option<String>,
    },
}

fn child<'a, 'input>(
    node: Node<'a, 'input>,
    name: &str,
) -> anyhow::Result<Option<Node<'a, 'input>>> {
    let mut matches = node
        .children()
        .filter(|n| n.is_element() && n.tag_name().name() == name);
    let first = matches.next();
    ensure!(matches.next().is_none(), "DLNA_DUPLICATE_FIELD");
    Ok(first)
}

pub(super) fn decode<T>(
    raw: &str,
    status: u16,
    service: &str,
    action: &str,
    read: fn(Node<'_, '_>) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    // Parser diagnostics can include attacker-controlled XML names. Keep errors bounded too.
    let doc = super::xml(raw).map_err(|_| anyhow::anyhow!("DLNA_INVALID_XML"))?;
    let root = doc.root_element();
    ensure!(
        root.has_tag_name((ENVELOPE, "Envelope")),
        "DLNA_INVALID_ENVELOPE"
    );
    let body = child(root, "Body")?.context("DLNA_BODY_MISSING")?;
    ensure!(
        body.tag_name().namespace() == Some(ENVELOPE),
        "DLNA_INVALID_ENVELOPE"
    );
    let mut replies = body.children().filter(Node::is_element);
    let reply = replies.next().context("DLNA_RESPONSE_MISSING")?;
    ensure!(replies.next().is_none(), "DLNA_AMBIGUOUS_RESPONSE");
    if reply.has_tag_name((ENVELOPE, "Fault")) {
        let code = reply
            .descendants()
            .find(|n| n.is_element() && n.tag_name().name() == "errorCode")
            .and_then(|n| n.text())
            .and_then(|s| s.trim().parse::<u16>().ok());
        anyhow::bail!(
            "DLNA_SOAP_FAULT: {}",
            code.map_or_else(|| "unknown".into(), |n| n.to_string())
        );
    }
    ensure!((200..300).contains(&status), "DLNA_HTTP_ERROR");
    ensure!(
        reply.has_tag_name((service, format!("{action}Response").as_str())),
        "DLNA_UNEXPECTED_RESPONSE"
    );
    read(reply)
}

fn field(node: Node<'_, '_>, name: &str, limit: usize) -> anyhow::Result<Option<String>> {
    let Some(node) = child(node, name)? else {
        return Ok(None);
    };
    ensure!(
        !node.children().any(|n| n.is_element()),
        "DLNA_INVALID_FIELD"
    );
    let mut texts = node
        .children()
        .filter(Node::is_text)
        .filter_map(|n| n.text());
    let mut value = std::borrow::Cow::Borrowed(texts.next().unwrap_or(""));
    ensure!(value.len() <= limit, "DLNA_FIELD_TOO_LARGE");
    for text in texts {
        ensure!(value.len() + text.len() <= limit, "DLNA_FIELD_TOO_LARGE");
        value.to_mut().push_str(text);
    }
    Ok(Some(value.trim().to_owned()))
}

pub(super) fn state(node: Node<'_, '_>) -> anyhow::Result<String> {
    field(node, "CurrentTransportState", 64)?
        .filter(|s| !s.is_empty())
        .context("DLNA_STATE_MISSING")
}

pub(super) fn position(node: Node<'_, '_>) -> anyhow::Result<CommandResult> {
    Ok(CommandResult::Position {
        relative_time: field(node, "RelTime", 32)?,
        track_duration: field(node, "TrackDuration", 32)?,
    })
}

pub(super) fn current_uri(node: Node<'_, '_>) -> anyhow::Result<String> {
    Ok(field(node, "CurrentURI", 8192)?.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events;
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    const SERVICE: &str = "urn:schemas-upnp-org:service:AVTransport:1";

    fn envelope(body: &str) -> String {
        format!("<s:Envelope xmlns:s=\"{ENVELOPE}\"><s:Body>{body}</s:Body></s:Envelope>")
    }

    fn response(action: &str, fields: &str) -> String {
        envelope(&format!(
            "<u:{action}Response xmlns:u=\"{SERVICE}\">{fields}</u:{action}Response>"
        ))
    }

    fn state_response(raw: &str) -> anyhow::Result<CommandResult> {
        decode(raw, 200, SERVICE, "GetTransportInfo", state)
            .map(|transport_state| CommandResult::State { transport_state })
    }

    #[test]
    fn large_soap_metadata_never_enters_the_host_mailbox() {
        let raw = response(
            "GetTransportInfo",
            &format!(
                "<CurrentTransportState>PLAYING</CurrentTransportState><metadata>{}</metadata>",
                "x".repeat(160 * 1024)
            ),
        );
        let stop = CancellationToken::new();
        let (output, inbox) = events::channel(4, stop.clone());
        events::event(
            &output,
            "dlna.state",
            json!({"response":state_response(&raw).unwrap(),"firstFrameMeasured":false}),
        );
        let event = inbox.try_recv().unwrap();
        assert!(!stop.is_cancelled());
        assert!(event.len() < 256);
        let event: serde_json::Value = serde_json::from_str(&event).unwrap();
        assert_eq!(
            event["body"]["response"],
            json!({"kind":"state","transportState":"PLAYING"})
        );

        let accepted = decode(
            &response(
                "Play",
                &format!("<extra>{}</extra>", "x".repeat(160 * 1024)),
            ),
            200,
            SERVICE,
            "Play",
            |_| Ok(CommandResult::Accepted),
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(accepted).unwrap(),
            json!({"kind":"accepted"})
        );
    }

    #[test]
    fn fields_and_fault_diagnostics_have_independent_bounds() {
        let stop = CancellationToken::new();
        let (output, inbox) = events::channel(4, stop.clone());
        let oversized = "x".repeat(160 * 1024);
        let state_error = state_response(&response(
            "GetTransportInfo",
            &format!("<CurrentTransportState>{oversized}</CurrentTransportState>"),
        ))
        .unwrap_err();
        assert_eq!(state_error.to_string(), "DLNA_FIELD_TOO_LARGE");
        let fault = envelope(&format!(
            "<s:Fault><detail><errorCode>{oversized}</errorCode></detail></s:Fault>"
        ));
        let fault_error = decode(&fault, 500, SERVICE, "Play", |_| Ok(())).unwrap_err();
        assert_eq!(fault_error.to_string(), "DLNA_SOAP_FAULT: unknown");
        let malformed = format!("<x:{oversized}/>");
        let xml_error = state_response(&malformed).unwrap_err();
        assert_eq!(xml_error.to_string(), "DLNA_INVALID_XML");
        for error in [state_error, fault_error, xml_error] {
            events::event(&output, "error", json!({"code":error.to_string()}));
            assert!(inbox.try_recv().unwrap().len() < 128);
        }
        assert!(!stop.is_cancelled());
        let fault = envelope(
            "<s:Fault><detail><UPnPError><errorCode>701</errorCode></UPnPError></detail></s:Fault>",
        );
        assert_eq!(
            decode(&fault, 500, SERVICE, "Play", |_| Ok(()))
                .unwrap_err()
                .to_string(),
            "DLNA_SOAP_FAULT: 701"
        );
    }

    #[test]
    fn state_is_scoped_to_the_expected_body_and_rejects_ambiguity() {
        let valid = response(
            "GetTransportInfo",
            "<CurrentTransportState> PLAYING </CurrentTransportState>",
        );
        assert!(state_response(&valid).is_ok());
        for raw in [
            valid.replace("GetTransportInfoResponse", "GetMediaInfoResponse"),
            valid.replace(SERVICE, "urn:other"),
            valid.replace("CurrentTransportState", "Unexpected"),
            valid.replace(" PLAYING ", ""),
            valid.replace(" PLAYING ", "<nested>PLAYING</nested>"),
            valid.replace("</u:GetTransportInfoResponse>", "<CurrentTransportState>STOPPED</CurrentTransportState></u:GetTransportInfoResponse>"),
            valid.replace("</s:Body>", "<another/></s:Body>"),
            response("GetTransportInfo", "").replace("<s:Body>", "<s:Header><CurrentTransportState>PLAYING</CurrentTransportState></s:Header><s:Body>"),
        ] {
            assert!(state_response(&raw).is_err(), "accepted {raw}");
        }
        assert_eq!(
            decode(&valid, 503, SERVICE, "GetTransportInfo", state)
                .unwrap_err()
                .to_string(),
            "DLNA_HTTP_ERROR"
        );
    }

    #[test]
    fn position_and_uri_preserve_values_with_explicit_limits() {
        let raw = response(
            "GetPositionInfo",
            "<RelTime>00:12:34</RelTime><TrackDuration>NOT_IMPLEMENTED</TrackDuration><TrackMetaData>ignored</TrackMetaData>",
        );
        let result = decode(&raw, 200, SERVICE, "GetPositionInfo", position).unwrap();
        assert_eq!(
            serde_json::to_value(result).unwrap(),
            json!({"kind":"position","relativeTime":"00:12:34","trackDuration":"NOT_IMPLEMENTED"})
        );
        assert!(
            decode(
                &response(
                    "GetPositionInfo",
                    &format!("<RelTime>{}</RelTime>", "0".repeat(33))
                ),
                200,
                SERVICE,
                "GetPositionInfo",
                position
            )
            .is_err()
        );
        let uri = "http://192.168.1.1/media/视频?a=1&b=\"'<>";
        let raw = response(
            "GetMediaInfo",
            &format!("<CurrentURI>{}</CurrentURI>", super::super::escape(uri)),
        );
        assert_eq!(
            decode(&raw, 200, SERVICE, "GetMediaInfo", current_uri).unwrap(),
            uri
        );
        assert_eq!(
            decode(
                &response("GetMediaInfo", "<CurrentURI/>"),
                200,
                SERVICE,
                "GetMediaInfo",
                current_uri
            )
            .unwrap(),
            ""
        );
        assert!(
            decode(
                &response(
                    "GetMediaInfo",
                    &format!("<CurrentURI>{}</CurrentURI>", "x".repeat(8193))
                ),
                200,
                SERVICE,
                "GetMediaInfo",
                current_uri
            )
            .is_err()
        );
    }

    #[test]
    fn fragmented_text_is_complete_and_shares_one_field_budget() {
        let raw = response(
            "GetTransportInfo",
            "<CurrentTransportState>PLAY<!-- comment -->ING</CurrentTransportState>",
        );
        assert_eq!(
            state_response(&raw).unwrap(),
            CommandResult::State {
                transport_state: "PLAYING".into()
            }
        );
        let raw = response(
            "GetMediaInfo",
            "<CurrentURI>http://192.168.1.1/<!-- comment -->media</CurrentURI>",
        );
        assert_eq!(
            decode(&raw, 200, SERVICE, "GetMediaInfo", current_uri).unwrap(),
            "http://192.168.1.1/media"
        );
        let raw = response(
            "GetTransportInfo",
            &format!(
                "<CurrentTransportState>{}<!-- split -->{}</CurrentTransportState>",
                "x".repeat(32),
                "x".repeat(33)
            ),
        );
        assert_eq!(
            state_response(&raw).unwrap_err().to_string(),
            "DLNA_FIELD_TOO_LARGE"
        );
    }
}
