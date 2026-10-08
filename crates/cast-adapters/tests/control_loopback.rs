//! Two real WSS peers, TLS pin, explicit receiver approval and stop/shutdown.
use cast_adapters::runtime::Engine;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sender_rejects_previous_receiver_before_sending_code() {
    use cast_adapters::{auth, protocol::Message};
    use futures_util::{SinkExt, StreamExt};
    let sender = Engine::new().unwrap();
    let identity = auth::Identity::create().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let fingerprint = identity.fingerprint.clone();
    let acceptor =
        tokio_rustls::TlsAcceptor::from(std::sync::Arc::new(identity.server_config().unwrap()));
    let peer = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let tls = acceptor.accept(stream).await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(tls).await.unwrap();
        let old_challenge = Message::new(
            "auth.challenge",
            json!({"nonce":auth::random_token(),"expiresInMs":30000}),
        );
        ws.send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::to_string(&old_challenge).unwrap().into(),
        ))
        .await
        .unwrap();
        let response = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .unwrap();
        if let Some(Ok(frame)) = response {
            assert!(
                !frame.is_text(),
                "A pairing code was sent to an incompatible receiver"
            );
        }
    });
    sender.command(json!({"op":"connect","address":address.to_string(),"fingerprint":fingerprint,"invite":"12345678"})).unwrap();
    assert_eq!(event(&sender, "error")["code"], "PAIRING_VERSION_MISMATCH");
    sender.close();
    peer.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn receiver_rejects_previous_sender_before_requesting_approval() {
    use cast_adapters::{auth, protocol::Message};
    use futures_util::{SinkExt, StreamExt};
    let receiver = Engine::new().unwrap();
    receiver
        .command(json!({"op":"listen","address":"127.0.0.1:0"}))
        .unwrap();
    let ready = event(&receiver, "receiver.ready");
    let config = auth::pinned_config(ready["fingerprint"].as_str().unwrap()).unwrap();
    let (mut ws, _) = tokio_tungstenite::connect_async_tls_with_config(
        format!("wss://{}/v1/ws", ready["address"].as_str().unwrap()),
        None,
        false,
        Some(tokio_tungstenite::Connector::Rustls(std::sync::Arc::new(
            config,
        ))),
    )
    .await
    .unwrap();
    let frame = ws.next().await.unwrap().unwrap();
    let challenge: Value = serde_json::from_str(frame.to_text().unwrap()).unwrap();
    assert_eq!(challenge["body"]["pairingVersion"], auth::PAIRING_VERSION);
    let identity = auth::Identity::create().unwrap();
    let old_pair = Message::new(
        "pair.request",
        json!({"invite":ready["invite"],"senderDeviceId":identity.id,"senderName":"previous sender","senderPublicKey":identity.public_key,"signature":identity.sign(challenge["body"]["nonce"].as_str().unwrap()).unwrap()}),
    );
    ws.send(tokio_tungstenite::tungstenite::Message::Text(
        serde_json::to_string(&old_pair).unwrap().into(),
    ))
    .await
    .unwrap();
    let response = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .unwrap();
    if let Some(Ok(frame)) = response {
        assert!(!frame.is_text(), "An incompatible sender was accepted");
    }
    while let Some(raw) = receiver.poll() {
        assert_ne!(
            serde_json::from_str::<Value>(&raw).unwrap()["type"],
            "pair.request"
        );
    }
    receiver.close();
}

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

fn stop_barrier(engine: &Engine) {
    engine.command(json!({"op":"stop"})).unwrap();
    // The final connection event must precede the stop acknowledgement.
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut disconnected = false;
    loop {
        if let Some(text) = engine.poll() {
            let value: Value = serde_json::from_str(&text).unwrap();
            if value["type"] == "disconnected" {
                disconnected = true;
            }
            if value["type"] == "stopped" {
                assert!(disconnected, "Stop acknowledged before connection teardown");
                break;
            }
        }
        assert!(Instant::now() < deadline, "Stop barrier timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn repeated_pairing_cancellation_releases_receiver_slots() {
    let receiver = Engine::new().unwrap();
    let sender = Engine::new().unwrap();
    receiver
        .command(json!({"op":"listen","address":"127.0.0.1:0"}))
        .unwrap();
    let ready = event(&receiver, "receiver.ready");
    // More than the eight simultaneous-connection limit, without a 60-second wait.
    for _ in 0..12 {
        receiver.command(json!({"op":"invite"})).unwrap();
        let invite = event(&receiver, "receiver.invite");
        sender.command(json!({"op":"connect","address":ready["address"],"fingerprint":ready["fingerprint"],"invite":invite["invite"]})).unwrap();
        let pending = event(&receiver, "pair.request");
        sender.command(json!({"op":"stop"})).unwrap();
        event(&sender, "stopped");
        let closed = event(&receiver, "pair.closed");
        assert_eq!(closed["connectionId"], pending["connectionId"]);
        assert_eq!(closed["reason"], "peer_closed");
        // A late click on a closed approval must not authorize the next attempt.
        receiver
            .command(json!({"op":"approve","connectionId":pending["connectionId"],"accept":true}))
            .unwrap();
    }
    sender.close();
    receiver.close();
}

#[test]
fn same_sender_can_stop_and_reconnect_without_old_connection_events() {
    let receiver = Engine::new().unwrap();
    let sender = Engine::new().unwrap();
    receiver
        .command(json!({"op":"listen","address":"127.0.0.1:0"}))
        .unwrap();
    let ready = event(&receiver, "receiver.ready");
    for _ in 0..3 {
        receiver.command(json!({"op":"invite"})).unwrap();
        let invite = event(&receiver, "receiver.invite");
        sender.command(json!({"op":"connect","address":ready["address"],"fingerprint":ready["fingerprint"],"invite":invite["invite"]})).unwrap();
        let pending = event(&receiver, "pair.request");
        receiver
            .command(json!({"op":"approve","connectionId":pending["connectionId"],"accept":true}))
            .unwrap();
        event(&sender, "connected");
        signal(
            &sender,
            "session.start",
            &Value::Null,
            json!({"mode":"file"}),
        );
        let accepted = message(&sender, "session.accepted");
        assert!(!accepted["sessionId"].is_null());
        event(&receiver, "session.started");
        stop_barrier(&sender);
        event(&receiver, "session.closed");
        assert!(sender.poll().is_none(), "Old connection emitted after stop");
    }
    sender.close();
    receiver.close();
}

#[test]
fn receiver_stop_rejects_pending_approval() {
    let receiver = Engine::new().unwrap();
    let sender = Engine::new().unwrap();
    receiver
        .command(json!({"op":"listen","address":"127.0.0.1:0"}))
        .unwrap();
    let ready = event(&receiver, "receiver.ready");
    sender.command(json!({"op":"connect","address":ready["address"],"fingerprint":ready["fingerprint"],"invite":ready["invite"]})).unwrap();
    let pending = event(&receiver, "pair.request");
    receiver.command(json!({"op":"stop"})).unwrap();
    let closed = event(&receiver, "pair.closed");
    assert_eq!(closed["connectionId"], pending["connectionId"]);
    assert_eq!(closed["reason"], "rejected");
    assert_eq!(event(&sender, "error")["code"], "PAIR_REJECTED");
    sender.close();
    receiver.close();
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
