//! Shared bounded host event sink. Transport modules never depend on Runtime.
use serde_json::{Value, json};
use std::sync::mpsc::SyncSender;

pub(crate) type Events = SyncSender<String>;
pub(crate) fn emit(events: &Events, value: Value) {
    let _ = events.try_send(value.to_string());
}
pub(crate) fn event(events: &Events, kind: &str, body: Value) {
    emit(events, json!({"type":kind,"body":body}));
}
