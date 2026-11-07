use serde::{
    Deserialize, Serialize,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use serde_json::Value;
use std::fmt;
use uuid::Uuid;

pub const MAX_FRAME: usize = 128 * 1024;
// Reserve room for the host envelope and receiver-owned fingerprint enrichment.
pub const MAX_MESSAGE: usize = MAX_FRAME - 1024;
pub const MAX_SDP: usize = 96 * 1024;
pub const MAX_ICE: usize = 4096;

pub fn parse_command(text: &str) -> anyhow::Result<Value> {
    let value = parse_json(text)?;
    anyhow::ensure!(
        value.is_object() && value["op"].is_string(),
        "INVALID_COMMAND"
    );
    Ok(value)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Message {
    pub version: u32,
    pub id: Uuid,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub session_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<Uuid>,
    pub body: Value,
}

impl Message {
    pub fn new(kind: &str, body: Value) -> Self {
        Self {
            version: 1,
            id: Uuid::new_v4(),
            kind: kind.into(),
            session_id: None,
            reply_to: None,
            body,
        }
    }
    pub fn reply(&self, kind: &str, body: Value) -> Self {
        let mut result = Self::new(kind, body);
        result.reply_to = Some(self.id);
        result.session_id = self.session_id;
        result
    }
    pub fn error(&self, code: &str) -> Self {
        self.reply("error", serde_json::json!({"code":code,"retryable":false}))
    }
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let value: Self = serde_json::from_value(parse_json(text)?)?;
        value.validate()?;
        Ok(value)
    }
    pub(crate) fn from_value(value: Value) -> anyhow::Result<Self> {
        // A Value cannot contain duplicate keys; move it into the typed message.
        let message: Self = serde_json::from_value(value)?;
        message.validate()?;
        Ok(message)
    }
    fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(self.version == 1, "VERSION_UNSUPPORTED");
        anyhow::ensure!(self.body.is_object(), "INVALID_BODY");
        if matches!(self.kind.as_str(), "rtc.offer" | "rtc.answer") {
            anyhow::ensure!(self.string("sdp")?.len() <= MAX_SDP, "SDP_TOO_LARGE");
            Uuid::parse_str(self.string("negotiationId")?)?;
        }
        if self.kind == "rtc.ice" {
            anyhow::ensure!(self.string("candidate")?.len() <= MAX_ICE, "ICE_TOO_LARGE");
            anyhow::ensure!(
                self.body["sdpMLineIndex"]
                    .as_u64()
                    .is_some_and(|n| n <= 65535),
                "INVALID_ICE"
            );
            Uuid::parse_str(self.string("negotiationId")?)?;
        }
        if self.kind == "rtc.state" {
            Uuid::parse_str(self.string("negotiationId")?)?;
            anyhow::ensure!(
                matches!(
                    self.string("state")?,
                    "connected" | "disconnected" | "failed"
                ),
                "INVALID_RTC_STATE"
            );
        }
        // Raw text length is insufficient: numbers, UUIDs and omitted fields
        // may grow when serialized. Both entry points use the canonical size.
        json_size(self, MAX_MESSAGE)?;
        Ok(())
    }
    pub fn string(&self, field: &str) -> anyhow::Result<&str> {
        self.body[field]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("INVALID_FIELD: {field}"))
    }
}

/// Count escaped wire bytes without allocating a second serialized copy.
pub(crate) fn json_size(value: &impl Serialize, limit: usize) -> serde_json::Result<usize> {
    let mut writer = LimitedWriter::new(std::io::sink(), limit);
    serde_json::to_writer(&mut writer, value)?;
    Ok(writer.written)
}

/// Stop serialization at the budget instead of first allocating an oversized string.
pub(crate) fn json_text(value: &impl Serialize, limit: usize) -> serde_json::Result<String> {
    let mut writer = LimitedWriter::new(Vec::new(), limit);
    serde_json::to_writer(&mut writer, value)?;
    Ok(String::from_utf8(writer.inner).expect("JSON serialization produces UTF-8"))
}

struct LimitedWriter<W> {
    inner: W,
    written: usize,
    limit: usize,
}
impl<W> LimitedWriter<W> {
    fn new(inner: W, limit: usize) -> Self {
        Self {
            inner,
            written: 0,
            limit,
        }
    }
}
impl<W: std::io::Write> std::io::Write for LimitedWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.limit - self.written {
            return Err(std::io::Error::other("FRAME_TOO_LARGE"));
        }
        let written = self.inner.write(bytes)?;
        self.written += written;
        Ok(written)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

fn parse_json(text: &str) -> anyhow::Result<Value> {
    anyhow::ensure!(text.len() <= MAX_FRAME, "FRAME_TOO_LARGE");
    let mut decoder = serde_json::Deserializer::from_str(text);
    let value = Unique::deserialize(&mut decoder)?.0;
    decoder.end()?;
    Ok(value)
}

// Build the validated tree once. The object itself detects duplicate decoded
// keys; no second parse or parallel HashSet of key allocations is needed.
struct Unique(Value);
impl<'de> Deserialize<'de> for Unique {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Unique;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("bounded JSON with unique keys")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Unique, M::Error> {
                let mut object = serde_json::Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    if object.contains_key(&key) {
                        return Err(de::Error::custom("duplicate key"));
                    }
                    object.insert(key, map.next_value::<Unique>()?.0);
                }
                Ok(Unique(Value::Object(object)))
            }
            fn visit_seq<S: SeqAccess<'de>>(self, mut seq: S) -> Result<Unique, S::Error> {
                let mut array = Vec::new();
                while let Some(value) = seq.next_element::<Unique>()? {
                    array.push(value.0);
                }
                Ok(Unique(Value::Array(array)))
            }
            fn visit_bool<E: de::Error>(self, value: bool) -> Result<Unique, E> {
                Ok(Unique(value.into()))
            }
            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Unique, E> {
                Ok(Unique(value.into()))
            }
            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Unique, E> {
                Ok(Unique(value.into()))
            }
            fn visit_f64<E: de::Error>(self, n: f64) -> Result<Unique, E> {
                if !n.is_finite() || n.abs() > 9_007_199_254_740_991.0 {
                    return Err(E::custom("number out of range"));
                }
                Ok(Unique(n.into()))
            }
            fn visit_str<E: de::Error>(self, value: &str) -> Result<Unique, E> {
                Ok(Unique(value.into()))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Unique, E> {
                Ok(Unique(Value::Null))
            }
        }
        d.deserialize_any(V)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoded_messages_reserve_space_for_host_delivery() {
        let mut message = Message::new("session.state", serde_json::json!({"padding":""}));
        let overhead = serde_json::to_string(&message).unwrap().len();
        message.body["padding"] = Value::String("x".repeat(MAX_FRAME - overhead));
        let wire = serde_json::to_string(&message).unwrap();
        assert_eq!(wire.len(), MAX_FRAME);
        assert!(
            Message::parse(&wire).is_err(),
            "no room for the host envelope"
        );
        assert!(Message::from_value(serde_json::to_value(message).unwrap()).is_err());
    }

    #[test]
    fn decoded_number_expansion_is_bounded() {
        let wire = serde_json::to_string(&Message::new("session.state", serde_json::json!({})))
            .unwrap()
            .replace(
                r#""body":{}"#,
                &format!(r#""body":{{"values":[{}0]}}"#, "1e15,".repeat(10_000)),
            );
        assert!(wire.len() < MAX_FRAME);
        assert!(
            Message::parse(&wire).is_err(),
            "compact numbers expand past the event limit"
        );
    }

    #[test]
    fn recursive_values_keep_their_types_and_contents() {
        let body = r#"{"text":"电视\n\u7535\"\\","nested":[null,true,false,-1,1.25,-0.0,18446744073709551615,{"key":[] }]}"#;
        let expected: Value = serde_json::from_str(body).unwrap();
        let command = format!(r#"{{"op":"signal","message":{body}}}"#);
        assert_eq!(parse_command(&command).unwrap()["message"], expected);
        let message = Message::new("ping", expected.clone());
        let parsed = Message::parse(&serde_json::to_string(&message).unwrap()).unwrap();
        assert_eq!(parsed.body, expected);
        assert_eq!(parsed.id, message.id);
    }

    #[test]
    fn both_text_entries_reject_nested_ambiguity_and_invalid_numbers() {
        for body in [
            r#"{"items":[{"key":1,"\u006bey":2}]}"#,
            r#"{"items":[{"nested":{"x":null,"x":false}}]}"#,
            r#"{"number":1e100}"#,
            r#"{"number":1e999}"#,
        ] {
            let command = format!(r#"{{"op":"signal","message":{body}}}"#);
            assert!(parse_command(&command).is_err(), "accepted {body}");
            let wire = serde_json::to_string(&Message::new("ping", serde_json::json!({})))
                .unwrap()
                .replace(r#""body":{}"#, &format!(r#""body":{body}"#));
            assert!(Message::parse(&wire).is_err(), "accepted {body}");
        }
        assert!(parse_command(r#"{"op":"stop"} null"#).is_err());
        let deep = format!(
            r#"{{"op":"signal","body":{}0{}}}"#,
            "[".repeat(128),
            "]".repeat(128)
        );
        assert!(parse_command(&deep).is_err());
        let oversized = format!(r#"{{"op":"{}"}}"#, "x".repeat(MAX_FRAME));
        assert!(parse_command(&oversized).is_err());
    }
    #[test]
    fn rejects_ambiguous_json() {
        let msg = Message::new("ping", serde_json::json!({}));
        let wire = serde_json::to_string(&msg).unwrap();
        assert!(Message::parse(&wire).is_ok());
        assert!(
            Message::parse(&wire.replace("\"body\":{}", "\"body\":{\"x\":1,\"x\":2}")).is_err()
        );
        assert!(Message::parse(&wire.replace("\"version\":1", "\"version\":2")).is_err());
        assert!(Message::parse(&format!("{wire} null")).is_err());
    }
    #[test]
    fn rejects_huge_sdp() {
        let msg = Message::new(
            "rtc.offer",
            serde_json::json!({"sdp":"x".repeat(MAX_SDP+1),"negotiationId":Uuid::new_v4()}),
        );
        assert!(Message::parse(&serde_json::to_string(&msg).unwrap()).is_err());
        assert!(Message::from_value(serde_json::to_value(msg).unwrap()).is_err());
    }
}
