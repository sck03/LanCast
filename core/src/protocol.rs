use serde::{
    Deserialize, Serialize,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use serde_json::Value;
use std::{collections::HashSet, fmt};
use uuid::Uuid;

pub const MAX_FRAME: usize = 128 * 1024;
pub const MAX_SDP: usize = 96 * 1024;
pub const MAX_ICE: usize = 4096;

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
        anyhow::ensure!(text.len() <= MAX_FRAME, "FRAME_TOO_LARGE");
        // serde_json::Value normally accepts duplicate keys; validate recursively first.
        let mut deserializer = serde_json::Deserializer::from_str(text);
        Unique::deserialize(&mut deserializer)?;
        deserializer.end()?;
        let value: Self = serde_json::from_str(text)?;
        anyhow::ensure!(value.version == 1, "VERSION_UNSUPPORTED");
        anyhow::ensure!(value.body.is_object(), "INVALID_BODY");
        if matches!(value.kind.as_str(), "rtc.offer" | "rtc.answer") {
            anyhow::ensure!(value.string("sdp")?.len() <= MAX_SDP, "SDP_TOO_LARGE");
            Uuid::parse_str(value.string("negotiationId")?)?;
        }
        if value.kind == "rtc.ice" {
            anyhow::ensure!(value.string("candidate")?.len() <= MAX_ICE, "ICE_TOO_LARGE");
            anyhow::ensure!(
                value.body["sdpMLineIndex"]
                    .as_u64()
                    .is_some_and(|n| n <= 65535),
                "INVALID_ICE"
            );
            Uuid::parse_str(value.string("negotiationId")?)?;
        }
        Ok(value)
    }
    pub fn string(&self, field: &str) -> anyhow::Result<&str> {
        self.body[field]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("INVALID_FIELD: {field}"))
    }
}

struct Unique;
impl<'de> Deserialize<'de> for Unique {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Unique;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("bounded JSON with unique keys")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Unique, M::Error> {
                let mut keys = HashSet::new();
                while let Some(key) = map.next_key::<String>()? {
                    if !keys.insert(key) {
                        return Err(de::Error::custom("duplicate key"));
                    }
                    map.next_value::<Unique>()?;
                }
                Ok(Unique)
            }
            fn visit_seq<S: SeqAccess<'de>>(self, mut seq: S) -> Result<Unique, S::Error> {
                while seq.next_element::<Unique>()?.is_some() {}
                Ok(Unique)
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> Result<Unique, E> {
                Ok(Unique)
            }
            fn visit_i64<E: de::Error>(self, _: i64) -> Result<Unique, E> {
                Ok(Unique)
            }
            fn visit_u64<E: de::Error>(self, _: u64) -> Result<Unique, E> {
                Ok(Unique)
            }
            fn visit_f64<E: de::Error>(self, n: f64) -> Result<Unique, E> {
                if !n.is_finite() || n.abs() > 9_007_199_254_740_991.0 {
                    return Err(E::custom("number out of range"));
                }
                Ok(Unique)
            }
            fn visit_str<E: de::Error>(self, _: &str) -> Result<Unique, E> {
                Ok(Unique)
            }
            fn visit_unit<E: de::Error>(self) -> Result<Unique, E> {
                Ok(Unique)
            }
        }
        d.deserialize_any(V)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    }
}
