//! Sender control connection and RTC recovery coordination.
use super::{
    heartbeat::Heartbeat,
    wire::{receive, send, wire_config},
};
use crate::{
    auth::{self, Identity},
    events::{self, Events, event},
    protocol::Message,
    rtc_recovery::{CAPABILITY, RtcRecovery},
};
use anyhow::{Context, ensure};
use serde_json::json;
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::{sync::mpsc, task::JoinHandle, time::Instant};
use tokio_tungstenite::{Connector, WebSocketStream};
use tokio_util::sync::CancellationToken;

/// Owns the worker as well as its queue. Closing is a barrier: after it returns,
/// this connection can no longer publish host events or touch a new session.
pub(crate) struct Client {
    sender: mpsc::Sender<Message>,
    cancel: CancellationToken,
    task: Option<JoinHandle<()>>,
}
impl Client {
    pub(crate) fn send(&self, message: Message) -> anyhow::Result<()> {
        let result = self
            .sender
            .try_send(message)
            .context("SIGNAL_QUEUE_FULL_OR_CLOSED");
        if result.is_err() {
            // A dropped offer/ICE/stop cannot leave a healthy-looking session.
            self.cancel.cancel();
        }
        result
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.sender.is_closed()
    }

    pub(crate) async fn close(&mut self) {
        self.cancel.cancel();
        if let Some(task) = &mut self.task {
            let _ = task.await;
        }
        self.task = None;
    }

    pub(crate) async fn connect(
        identity: Arc<Identity>,
        address: &str,
        pin: &str,
        invite: &str,
        name: &str,
        events: Events,
    ) -> anyhow::Result<Self> {
        let endpoint: SocketAddr = address.parse()?;
        ensure!(!endpoint.ip().is_unspecified(), "INVALID_ADDRESS");
        ensure!(auth::valid_pairing_code(invite), "INVALID_PAIRING_CODE");
        let config = auth::pinned_config(pin)?;
        let url = format!("wss://{endpoint}/v1/ws");
        let (mut ws, _) = tokio::time::timeout(
            Duration::from_secs(8),
            tokio_tungstenite::connect_async_tls_with_config(
                url,
                Some(wire_config()),
                false,
                Some(Connector::Rustls(Arc::new(config))),
            ),
        )
        .await??;
        let challenge = tokio::time::timeout(Duration::from_secs(30), receive(&mut ws)).await??;
        ensure!(challenge.kind == "auth.challenge", "AUTH_REQUIRED");
        ensure!(
            challenge.body["pairingVersion"].as_u64() == Some(auth::PAIRING_VERSION),
            "PAIRING_VERSION_MISMATCH"
        );
        let pair = Message::new(
            "pair.request",
            json!({"pairingVersion":auth::PAIRING_VERSION,"invite":invite,"senderDeviceId":identity.id,"senderName":name,"senderPublicKey":identity.public_key,"signature":identity.sign(challenge.string("nonce")?)?}),
        );
        send(&mut ws, &pair).await?;
        event(&events, "pair.waiting", json!({}));
        let accepted = tokio::time::timeout(Duration::from_secs(65), receive(&mut ws)).await??;
        ensure!(accepted.kind == "pair.accepted", "PAIR_REJECTED");
        ensure!(
            accepted.body["pairingVersion"].as_u64() == Some(auth::PAIRING_VERSION),
            "PAIRING_VERSION_MISMATCH"
        );
        event(
            &events,
            "connected",
            json!({"address":address,"trust":"session_only"}),
        );
        Ok(Self::spawn(ws, events))
    }

    fn spawn<S>(mut ws: WebSocketStream<S>, events: Events) -> Self
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let (tx, mut rx) = mpsc::channel::<Message>(32);
        let cancel = CancellationToken::new();
        let worker_cancel = cancel.clone();
        let task = tokio::spawn(async move {
            let epoch = Instant::now();
            let mut heartbeat = Heartbeat::new();
            let timer = tokio::time::sleep_until(heartbeat.deadline());
            tokio::pin!(timer);
            let mut recovery: Option<RtcRecovery> = None;
            let mut pending_recovery = None;
            let run = async {
                loop {
                    if rx.is_closed() {
                        break;
                    }
                    heartbeat.check()?;
                    if let Some(message) = recovery
                        .as_mut()
                        .and_then(|r| r.poll(epoch.elapsed().as_millis() as u64))
                    {
                        if message.kind == "session.stop" {
                            send(&mut ws, &message).await?;
                            recovery = None;
                        }
                        events::message(&events, &message);
                    }
                    let deadline = recovery
                        .as_ref()
                        .and_then(RtcRecovery::next_deadline)
                        .map(|ms| epoch + Duration::from_millis(ms))
                        .map_or(heartbeat.deadline(), |deadline| {
                            deadline.min(heartbeat.deadline())
                        });
                    timer.as_mut().reset(deadline);
                    tokio::select! {
                        request = rx.recv() => {
                            let Some(request) = request else { break };
                            let now = epoch.elapsed().as_millis() as u64;
                            if request.kind == "rtc.state" {
                                if let Some(r) = recovery.as_mut() { r.report(&request, false, now); }
                                else if request.body["state"] != "connected" {
                                    let stop = request.reply("session.stop", json!({"reason":"RTC_CONNECTION_LOST"}));
                                    send(&mut ws, &stop).await?;
                                    events::message(&events, &stop);
                                }
                                continue;
                            }
                            if request.kind == "session.start" {
                                pending_recovery = (request.body["mode"] == "mirror" && request.body["rtcRecovery"] == CAPABILITY).then_some(request.id);
                                recovery = None;
                            }
                            if request.kind == "rtc.offer" && let Some(r) = recovery.as_mut() { r.offer(&request); }
                            if request.kind == "rtc.ice" && recovery.as_ref().is_some_and(|r| !r.accepts(&request)) { continue; }
                            if request.kind == "session.stop" { recovery = None; pending_recovery = None; }
                            send(&mut ws, &request).await?;
                        }
                        response = receive(&mut ws) => {
                            let response = response?;
                            heartbeat.received()?;
                            let now = epoch.elapsed().as_millis() as u64;
                            if response.kind == "ping" {
                                send(&mut ws, &response.reply("pong", response.body.clone())).await?;
                                continue;
                            }
                            if response.kind == "session.accepted" && pending_recovery.is_some() && response.reply_to == pending_recovery {
                                if response.body["rtcRecovery"] == CAPABILITY {
                                    recovery = Some(RtcRecovery::new(response.session_id.context("SESSION_REQUIRED")?, now));
                                }
                                pending_recovery = None;
                            }
                            if response.kind == "rtc.state" {
                                if let Some(r) = recovery.as_mut() { r.report(&response, true, now); }
                                continue;
                            }
                            if matches!(response.kind.as_str(), "rtc.answer" | "rtc.ice") && recovery.as_ref().is_some_and(|r| !r.accepts(&response)) { continue; }
                            if response.kind == "session.stop" || response.kind == "error" { recovery = None; }
                            events::message(&events, &response);
                        }
                        _ = &mut timer => {}
                    }
                }
                Ok(())
            };
            // Cancellation wraps the entire loop, including a blocked socket write.
            let result: anyhow::Result<()> = tokio::select! { biased;
                _ = worker_cancel.cancelled() => Ok(()),
                result = run => result,
            };
            event(
                &events,
                "disconnected",
                json!({"reason":if result.is_err(){"connection_lost"}else{"closed"},"requiresNewConsent":true}),
            );
        });
        Self {
            sender: tx,
            cancel,
            task: Some(task),
        }
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{FutureExt, SinkExt};
    use tokio_tungstenite::tungstenite::protocol::Role;

    async fn test_connection() -> (
        Client,
        WebSocketStream<tokio::io::DuplexStream>,
        crate::events::EventInbox,
    ) {
        let (client, peer) = tokio::io::duplex(4096);
        let client =
            WebSocketStream::from_raw_socket(client, Role::Client, Some(wire_config())).await;
        let peer = WebSocketStream::from_raw_socket(peer, Role::Server, Some(wire_config())).await;
        let (events, output) = crate::events::channel(16, CancellationToken::new());
        (Client::spawn(client, events), peer, output)
    }

    async fn heartbeat(peer: &mut WebSocketStream<tokio::io::DuplexStream>) {
        let ping = Message::new("ping", json!({}));
        send(peer, &ping).await.unwrap();
        let pong = receive(peer).await.unwrap();
        assert_eq!(pong.kind, "pong");
        assert_eq!(pong.reply_to, Some(ping.id));
    }

    fn host_message(output: &crate::events::EventInbox) -> Message {
        let event: serde_json::Value = serde_json::from_str(&output.try_recv().unwrap()).unwrap();
        assert_eq!(event["type"], "message");
        serde_json::from_value(event["body"].clone()).unwrap()
    }

    #[tokio::test(start_paused = true)]
    async fn heartbeat_deadline_resets_only_on_received_messages() {
        let (mut client, mut peer, output) = test_connection().await;
        heartbeat(&mut peer).await;
        tokio::time::advance(Duration::from_secs(14)).await;
        heartbeat(&mut peer).await;
        tokio::time::advance(Duration::from_secs(14)).await;
        client.send(Message::new("ping", json!({}))).unwrap();
        assert_eq!(receive(&mut peer).await.unwrap().kind, "ping");
        assert!(!client.is_closed());
        assert!(output.try_recv().is_err());
        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        assert!(client.is_closed());
        let event: serde_json::Value = serde_json::from_str(&output.try_recv().unwrap()).unwrap();
        assert_eq!(event["type"], "disconnected");
        assert_eq!(event["body"]["reason"], "connection_lost");
        client.close().await;
    }

    #[tokio::test(start_paused = true)]
    async fn recovery_deadlines_survive_replayed_offer_and_old_success_reports() {
        let (mut client, mut peer, output) = test_connection().await;
        let start = Message::new(
            "session.start",
            json!({"mode":"mirror","rtcRecovery":CAPABILITY}),
        );
        client.send(start.clone()).unwrap();
        assert_eq!(receive(&mut peer).await.unwrap().id, start.id);
        let session = uuid::Uuid::new_v4();
        let mut accepted = start.reply("session.accepted", json!({"rtcRecovery":CAPABILITY}));
        accepted.session_id = Some(session);
        send(&mut peer, &accepted).await.unwrap();
        heartbeat(&mut peer).await;
        assert_eq!(host_message(&output).kind, "session.accepted");
        let mut offer = Message::new(
            "rtc.offer",
            json!({"sdp":"test","negotiationId":uuid::Uuid::new_v4()}),
        );
        offer.session_id = Some(session);
        client.send(offer.clone()).unwrap();
        assert_eq!(receive(&mut peer).await.unwrap().id, offer.id);

        // An off-grid failure verifies the deadline is relative to the report,
        // rather than rounded to a periodic polling tick.
        tokio::time::advance(Duration::from_millis(123)).await;
        let mut state = Message::new(
            "rtc.state",
            json!({"state":"failed","negotiationId":offer.body["negotiationId"]}),
        );
        state.session_id = Some(session);
        client.send(state.clone()).unwrap();
        client.send(Message::new("ping", json!({}))).unwrap();
        assert_eq!(receive(&mut peer).await.unwrap().kind, "ping");
        tokio::time::advance(Duration::from_millis(999)).await;
        tokio::task::yield_now().await;
        assert!(output.try_recv().is_err());
        tokio::time::advance(Duration::from_millis(1)).await;
        tokio::task::yield_now().await;
        assert_eq!(host_message(&output).body["attempt"], 1);

        client.send(offer.clone()).unwrap();
        assert_eq!(receive(&mut peer).await.unwrap().id, offer.id);
        state.body["state"] = json!("connected");
        client.send(state.clone()).unwrap();
        send(&mut peer, &state).await.unwrap();
        heartbeat(&mut peer).await;
        for (delay, attempt) in [(4, 2), (5, 3)] {
            tokio::time::advance(Duration::from_secs(delay)).await;
            tokio::task::yield_now().await;
            let restart = host_message(&output);
            assert_eq!(restart.kind, "rtc.restart");
            assert_eq!(restart.body["attempt"], attempt);
        }
        tokio::time::advance(Duration::from_secs(5)).await;
        let stop = receive(&mut peer).await.unwrap();
        assert_eq!(stop.kind, "session.stop");
        assert_eq!(stop.body["reason"], "RTC_RECOVERY_EXHAUSTED");
        assert_eq!(host_message(&output).id, stop.id);
        assert!(!client.is_closed());
        client.close().await;
    }

    #[tokio::test]
    async fn interrupted_close_keeps_the_worker_until_cleanup_finishes() {
        let (sender, _receiver) = mpsc::channel(1);
        let cancel = CancellationToken::new();
        let worker_cancel = cancel.clone();
        let (events, output) = crate::events::channel(4, CancellationToken::new());
        let mut client = Client {
            sender,
            cancel,
            task: Some(tokio::spawn(async move {
                worker_cancel.cancelled().await;
                tokio::task::yield_now().await;
                event(&events, "disconnected", json!({"reason":"closed"}));
            })),
        };
        // The command owner can cancel a reconnect while joining the previous client.
        assert!(client.close().now_or_never().is_none());
        assert!(client.task.is_some());
        assert!(output.try_recv().is_err());
        client.close().await;
        assert!(client.task.is_none());
        assert!(output.try_recv().unwrap().contains("disconnected"));
        client.close().await;
        assert_eq!(
            output.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Disconnected)
        );
    }

    #[tokio::test]
    async fn oversized_remote_message_closes_only_its_connection() {
        let (client, peer) = tokio::io::duplex(1024);
        let client =
            WebSocketStream::from_raw_socket(client, Role::Client, Some(wire_config())).await;
        let mut peer =
            WebSocketStream::from_raw_socket(peer, Role::Server, Some(wire_config())).await;
        let shutdown = CancellationToken::new();
        let (events, output) = crate::events::channel(4, shutdown.clone());
        let mut client = Client::spawn(client, events);
        // About 50 KiB on the wire, but over 160 KiB when normalized.
        let wire = serde_json::to_string(&Message::new("session.state", json!({})))
            .unwrap()
            .replace(
                r#""body":{}"#,
                &format!(r#""body":{{"values":[{}0]}}"#, "1e15,".repeat(10_000)),
            );
        peer.send(tokio_tungstenite::tungstenite::Message::Text(wire.into()))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while !client.is_closed() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        client.close().await;
        assert!(
            !shutdown.is_cancelled(),
            "peer input terminated the entire engine"
        );
        let event: serde_json::Value = serde_json::from_str(&output.try_recv().unwrap()).unwrap();
        assert_eq!(event["type"], "disconnected");
        assert_eq!(event["body"]["reason"], "connection_lost");
        assert_eq!(
            output.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Disconnected)
        );
    }

    #[test]
    fn signal_delivery_failure_cancels_the_connection() {
        for closed in [false, true] {
            let (sender, receiver) = mpsc::channel(1);
            let client = Client {
                sender,
                cancel: CancellationToken::new(),
                task: None,
            };
            client.send(Message::new("ping", json!({}))).unwrap();
            if closed {
                drop(receiver);
            }
            assert!(
                client
                    .send(Message::new("session.stop", json!({})))
                    .is_err()
            );
            assert!(
                client.cancel.is_cancelled(),
                "undelivered signal left the connection alive"
            );
        }
    }

    #[tokio::test]
    async fn close_interrupts_a_blocked_write_and_finishes_event_delivery() {
        // The peer keeps its end open but never reads. One signal fills the pipe.
        use tokio::io::AsyncReadExt;
        let (stream, mut unresponsive_peer) = tokio::io::duplex(16);
        let ws = WebSocketStream::from_raw_socket(stream, Role::Client, Some(wire_config())).await;
        let (events, output) = crate::events::channel(16, CancellationToken::new());
        let mut client = Client::spawn(ws, events);
        client
            .send(Message::new("session.start", json!({"mode":"file"})))
            .unwrap();
        // Observe the start of a write, then stop consuming it.
        unresponsive_peer.read_exact(&mut [0u8; 1]).await.unwrap();
        tokio::time::timeout(Duration::from_secs(1), client.close())
            .await
            .unwrap();
        let final_event: serde_json::Value =
            serde_json::from_str(&output.try_recv().unwrap()).unwrap();
        assert_eq!(final_event["type"], "disconnected");
        assert_eq!(final_event["body"]["reason"], "closed");
        assert_eq!(
            output.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Disconnected)
        );
    }
}
