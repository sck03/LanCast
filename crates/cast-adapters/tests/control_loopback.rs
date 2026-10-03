//! Two real WSS peers, TLS pin, explicit receiver approval and stop/shutdown.
use cast_adapters::runtime::Engine;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

fn event(engine: &Engine, kind: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if let Some(text) = engine.poll() {
            let value: Value = serde_json::from_str(&text).unwrap();
            if value["type"] == kind {
                return value["body"].clone();
            }
            assert_ne!(value["type"], "error", "Unexpected control error");
        }
        assert!(Instant::now() < deadline, "Timed out waiting for {kind}");
        std::thread::sleep(Duration::from_millis(10));
    }
}
#[test]
fn pinned_pairing_and_nonblocking_stop_during_pending_approval() {
    let receiver = Engine::new().unwrap();
    let sender = Engine::new().unwrap();
    receiver.command(json!({"op":"listen","address":"127.0.0.1:0","variant":"standard","name":"LanCast test"})).unwrap();
    let ready = event(&receiver, "receiver.ready");
    sender.command(json!({"op":"connect","address":ready["address"],"fingerprint":ready["fingerprint"],"invite":ready["invite"],"name":"synthetic-test"})).unwrap();
    let approval = event(&receiver, "pair.request");
    receiver
        .command(json!({"op":"approve","connectionId":approval["connectionId"],"accept":true}))
        .unwrap();
    event(&sender, "connected");
    sender.command(json!({"op":"stop"})).unwrap();
    event(&sender, "stopped");
    sender.close();
    receiver.close();

    // A network command waiting for approval must not block shutdown for 65 seconds.
    let receiver = Engine::new().unwrap();
    let sender = Engine::new().unwrap();
    receiver
        .command(json!({"op":"listen","address":"127.0.0.1:0","name":"LanCast cancellation test"}))
        .unwrap();
    let ready = event(&receiver, "receiver.ready");
    sender.command(json!({"op":"connect","address":ready["address"],"fingerprint":ready["fingerprint"],"invite":ready["invite"]})).unwrap();
    event(&receiver, "pair.request");
    let start = Instant::now();
    sender.close();
    assert!(start.elapsed() < Duration::from_secs(3));
    receiver.close();
}
