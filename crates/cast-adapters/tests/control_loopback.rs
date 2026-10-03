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

fn signal(engine: &Engine, kind: &str, session: &Value, body: Value) {
    let mut message = cast_adapters::protocol::Message::new(kind, body);
    message.session_id = session.as_str().map(|s| s.parse().unwrap());
    engine
        .command(json!({"op":"send", "message":message}))
        .unwrap();
}

fn message(engine: &Engine, kind: &str) -> Value {
    loop {
        let value = event(engine, "message");
        assert_ne!(value["type"], "error", "Unexpected protocol error: {value}");
        if value["type"] == kind {
            return value;
        }
    }
}

#[test]
fn negotiated_recovery_relays_new_transport_and_filters_stale_signals() {
    let receiver = Engine::new().unwrap();
    let sender = Engine::new().unwrap();
    receiver
        .command(json!({"op":"listen","address":"127.0.0.1:0"}))
        .unwrap();
    let ready = event(&receiver, "receiver.ready");
    sender.command(json!({"op":"connect","address":ready["address"],"fingerprint":ready["fingerprint"],"invite":ready["invite"]})).unwrap();
    let approval = event(&receiver, "pair.request");
    receiver
        .command(json!({"op":"approve","connectionId":approval["connectionId"],"accept":true}))
        .unwrap();
    event(&sender, "connected");
    signal(
        &sender,
        "session.start",
        &Value::Null,
        json!({"mode":"mirror","rtcRecovery":"replace-v1"}),
    );
    let accepted = message(&sender, "session.accepted");
    assert_eq!(accepted["body"]["rtcRecovery"], "replace-v1");
    let session = &accepted["sessionId"];
    let started = event(&receiver, "session.started");
    assert_eq!(started["rtcRecovery"], "replace-v1");
    let first = uuid::Uuid::new_v4();
    let second = uuid::Uuid::new_v4();
    signal(
        &sender,
        "rtc.offer",
        session,
        json!({"sdp":"synthetic-first", "negotiationId":first}),
    );
    message(&receiver, "rtc.offer");
    for engine in [&sender, &receiver] {
        signal(
            engine,
            "rtc.state",
            session,
            json!({"negotiationId":first,"state":"connected"}),
        );
    }
    signal(
        &sender,
        "rtc.state",
        session,
        json!({"negotiationId":first,"state":"failed"}),
    );
    let restart = message(&sender, "rtc.restart");
    assert_eq!(restart["body"]["attempt"], 1);
    signal(
        &sender,
        "rtc.offer",
        session,
        json!({"sdp":"synthetic-second","negotiationId":second,"previousNegotiationId":first}),
    );
    assert_eq!(
        message(&receiver, "rtc.offer")["body"]["negotiationId"],
        second.to_string()
    );
    signal(
        &receiver,
        "rtc.answer",
        session,
        json!({"sdp":"stale","negotiationId":first}),
    );
    signal(
        &receiver,
        "rtc.answer",
        session,
        json!({"sdp":"current","negotiationId":second}),
    );
    assert_eq!(message(&sender, "rtc.answer")["body"]["sdp"], "current");
    for id in [first, second] {
        signal(
            &sender,
            "rtc.ice",
            session,
            json!({"candidate":"synthetic","sdpMLineIndex":0,"negotiationId":id}),
        );
    }
    assert_eq!(
        message(&receiver, "rtc.ice")["body"]["negotiationId"],
        second.to_string()
    );
    for engine in [&sender, &receiver] {
        signal(
            engine,
            "rtc.state",
            session,
            json!({"negotiationId":second,"state":"connected"}),
        );
    }
    sender.command(json!({"op":"stop"})).unwrap();
    event(&sender, "stopped");
    event(&receiver, "session.closed");
    sender.close();
    receiver.close();
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

#[test]
#[cfg(target_os = "linux")]
fn rejected_file_command_closes_the_transferred_descriptor() {
    use std::os::fd::IntoRawFd;
    let file = tempfile::NamedTempFile::new().unwrap();
    let engine = Engine::new().unwrap();
    engine.close();
    let fd = file.reopen().unwrap().into_raw_fd();
    let path = format!("/proc/self/fd/{fd}");
    let before = std::fs::read_link(&path).unwrap();
    assert!(engine.command(json!({"op":"file.share","fd":fd})).is_err());
    if let Ok(now) = std::fs::read_link(path) {
        assert_ne!(now, before);
    }
}
