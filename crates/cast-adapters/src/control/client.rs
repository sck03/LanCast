//! Sender control connection and RTC recovery coordination.
use super::wire::{receive, send, wire_config};
use crate::{
    auth::{self, Identity},
    events::{Events, event},
    protocol::Message,
    rtc_recovery::{CAPABILITY, RtcRecovery},
};
use anyhow::{Context, ensure};
use serde_json::json;
use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{sync::mpsc, task::JoinHandle};
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
        self.sender
            .try_send(message)
            .context("SIGNAL_QUEUE_FULL_OR_CLOSED")
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.sender.is_closed()
    }

    pub(crate) async fn close(mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
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
            let mut last_seen = Instant::now();
            let mut tick = tokio::time::interval(Duration::from_millis(250));
            let mut recovery: Option<RtcRecovery> = None;
            let mut pending_recovery = None;
            let run = async {
                loop {
                    tokio::select! {
                        request = rx.recv() => {
                            let Some(request) = request else { break };
                            if rx.is_closed() { break; }
                            let now = epoch.elapsed().as_millis() as u64;
                            if request.kind == "rtc.state" {
                                if let Some(r) = recovery.as_mut() { r.report(&request, false, now); }
                                else if request.body["state"] != "connected" {
                                    let stop = request.reply("session.stop", json!({"reason":"RTC_CONNECTION_LOST"}));
                                    send(&mut ws, &stop).await?;
                                    event(&events, "message", serde_json::to_value(stop)?);
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
                            last_seen = Instant::now();
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
                            event(&events, "message", serde_json::to_value(response)?);
                        }
                        _ = tick.tick() => {
                            if rx.is_closed() { break; }
                            ensure!(last_seen.elapsed() < Duration::from_secs(15), "HEARTBEAT_TIMEOUT");
                            if let Some(message) = recovery.as_mut().and_then(|r| r.poll(epoch.elapsed().as_millis() as u64)) {
                                if message.kind == "session.stop" { send(&mut ws, &message).await?; recovery = None; }
                                event(&events, "message", serde_json::to_value(message)?);
                            }
                        }
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
    use tokio_tungstenite::tungstenite::protocol::Role;

    #[tokio::test]
    async fn close_interrupts_a_blocked_write_and_finishes_event_delivery() {
        // The peer keeps its end open but never reads. One signal fills the pipe.
        use tokio::io::AsyncReadExt;
        let (stream, mut unresponsive_peer) = tokio::io::duplex(16);
        let ws = WebSocketStream::from_raw_socket(stream, Role::Client, Some(wire_config())).await;
        let (events, output) = std::sync::mpsc::sync_channel(16);
        let client = Client::spawn(ws, events);
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
