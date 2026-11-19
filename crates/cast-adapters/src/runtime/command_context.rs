//! Correlation belongs to the local command boundary, including failed commands.
use serde_json::{Value, json};

pub(super) struct CommandContext {
    op: Value,
    request_id: Value,
    generation: Value,
}

impl CommandContext {
    pub(super) fn new(command: &Value) -> Self {
        Self {
            op: command["op"].clone(),
            request_id: command["requestId"].clone(),
            generation: command["generation"].clone(),
        }
    }

    pub(super) fn failure(&self, error: &anyhow::Error) -> Value {
        json!({"code": error.to_string(), "op": self.op,
            "requestId": self.request_id, "generation": self.generation})
    }
}
