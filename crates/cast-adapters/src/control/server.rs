//! Receiver pairing, session ownership and signaling. No platform media or UI types.
use super::wire::{receive, send, wire_config};
use crate::{
    auth::{self, Identity, Invitation},
    capabilities::Profile,
    events::{Events, event},
    protocol::Message,
    rtc_recovery::CAPABILITY,
    session::Session,
};
use anyhow::{Context, ensure};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    net::IpAddr,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    net::TcpListener,
    sync::{Mutex, Semaphore, mpsc, oneshot},
};
use tokio_tungstenite::WebSocketStream;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub(crate) struct Server {
    identity: Arc<Identity>,
    invitation: Mutex<Invitation>,
    pending: Mutex<HashMap<Uuid, oneshot::Sender<bool>>>,
    session: Mutex<Option<Session>>,
    connections: Mutex<HashMap<Uuid, Connection>>,
    events: Events,
    variant: String,
}

struct Connection {
    sender: mpsc::Sender<Message>,
    cancel: CancellationToken,
}

impl Server {
    pub(crate) fn new(identity: Arc<Identity>, variant: &str, events: Events) -> Self {
        Self {
            identity,
            invitation: Mutex::new(Invitation::new(Instant::now())),
            pending: Mutex::new(HashMap::new()),
            session: Mutex::new(None),
            connections: Mutex::new(HashMap::new()),
            events,
            variant: variant.into(),
        }
    }
    pub(crate) fn variant(&self) -> &str {
        &self.variant
    }
    pub(crate) async fn invitation(&self) -> String {
        self.invitation.lock().await.token().to_owned()
    }
    pub(crate) async fn renew_invitation(&self) -> String {
        let mut invitation = self.invitation.lock().await;
        *invitation = Invitation::new(Instant::now());
        invitation.token().to_owned()
    }
    pub(crate) async fn approve(&self, connection: Uuid, accepted: bool) {
        if let Some(tx) = self.pending.lock().await.remove(&connection) {
            let _ = tx.send(accepted);
        }
    }
    pub(crate) async fn send(&self, msg: Message) -> anyhow::Result<()> {
        let mut session = self.session.lock().await;
        let active = session.as_mut().context("NO_SESSION")?;
        ensure!(msg.session_id == Some(active.id), "SESSION_EXPIRED");
        ensure!(
            matches!(
                msg.kind.as_str(),
                "rtc.answer"
                    | "rtc.ice"
                    | "rtc.state"
                    | "statistics"
                    | "session.state"
                    | "session.stop"
                    | "error"
            ),
            "INVALID_DIRECTION"
        );
        if matches!(msg.kind.as_str(), "rtc.answer" | "rtc.ice" | "rtc.state")
            && msg.body["negotiationId"]
                .as_str()
                .and_then(|v| Uuid::parse_str(v).ok())
                != active.negotiation
        {
            return Ok(());
        }
        if msg.kind == "rtc.state" && !active.rtc_recovery {
            return Ok(());
        }
        if msg.kind == "session.state" && msg.body["state"] == "ready" {
            if active.rtc_recovery
                && msg.body["negotiationId"]
                    .as_str()
                    .and_then(|s| Uuid::parse_str(s).ok())
                    != active.negotiation
            {
                return Ok(());
            }
            active.ready()?;
        }
        let delivery = {
            let connections = self.connections.lock().await;
            let connection = connections
                .get(&active.owner)
                .context("CONTROL_CONNECTION_CLOSED")?;
            let result = connection
                .sender
                .try_send(msg.clone())
                .context("SIGNAL_QUEUE_FULL_OR_CLOSED");
            if result.is_err() {
                // Never retain a live session whose stop/signal could not be delivered.
                connection.cancel.cancel();
            }
            result
        };
        if msg.kind == "session.stop" {
            active.stop();
            *session = None;
        }
        delivery?;
        Ok(())
    }
    pub(crate) async fn stop(&self) {
        for (_, pending) in self.pending.lock().await.drain() {
            let _ = pending.send(false);
        }
        let mut guard = self.session.lock().await;
        if let Some(session) = guard.as_mut() {
            let mut message = Message::new("session.stop", json!({"reason":"receiver_stopped"}));
            message.session_id = Some(session.id);
            if let Some(connection) = self.connections.lock().await.get(&session.owner)
                && connection.sender.try_send(message).is_err()
            {
                connection.cancel.cancel();
            }
            session.stop();
        }
        *guard = None;
    }
    pub(crate) async fn serve(self: Arc<Self>, listener: TcpListener) -> anyhow::Result<()> {
        serve(listener, self).await
    }
}

// The handshake callback signature is imposed by tungstenite's public API.
#[allow(clippy::result_large_err)]
async fn serve(listener: TcpListener, server: Arc<Server>) -> anyhow::Result<()> {
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(server.identity.server_config()?));
    let slots = Arc::new(Semaphore::new(8));
    loop {
        let (stream, address) = listener.accept().await?;
        let Ok(permit) = slots.clone().try_acquire_owned() else {
            continue;
        };
        let acceptor = acceptor.clone();
        let server = server.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let handshake = async {
                let tls = acceptor.accept(stream).await?;
                let ws=tokio_tungstenite::accept_hdr_async_with_config(tls,|request:&tokio_tungstenite::tungstenite::handshake::server::Request,response:tokio_tungstenite::tungstenite::handshake::server::Response|{
            if request.uri().path()!="/v1/ws"||request.headers().contains_key("origin"){Err(tokio_tungstenite::tungstenite::http::Response::builder().status(403).body(Some("Native control endpoint".into())).unwrap())}else{Ok(response)}
        },Some(wire_config())).await?;
                Ok::<_, anyhow::Error>(ws)
            };
            if let Ok(Ok(ws)) = tokio::time::timeout(Duration::from_secs(8), handshake).await {
                let _ = receiver_connection(ws, address.ip(), server).await;
            }
        });
    }
}
async fn receiver_connection<
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
>(
    mut ws: WebSocketStream<S>,
    ip: IpAddr,
    server: Arc<Server>,
) -> anyhow::Result<()> {
    let nonce = auth::random_token();
    send(
        &mut ws,
        &Message::new("auth.challenge", json!({"nonce":nonce,"expiresInMs":30000})),
    )
    .await?;
    let pair = tokio::time::timeout(Duration::from_secs(30), receive(&mut ws)).await??;
    ensure!(pair.kind == "pair.request", "AUTH_REQUIRED");
    let owner = Uuid::parse_str(pair.string("senderDeviceId")?)?;
    let sender_name: String = pair.string("senderName")?.chars().take(80).collect();
    let public = pair.string("senderPublicKey")?;
    auth::verify(public, &nonce, owner, pair.string("signature")?)?;
    server
        .invitation
        .lock()
        .await
        .consume(pair.string("invite")?, Instant::now())?;
    let connection = Uuid::new_v4();
    let (tx, rx) = oneshot::channel();
    server.pending.lock().await.insert(connection, tx);
    event(
        &server.events,
        "pair.request",
        json!({"connectionId":connection,"senderDeviceId":owner,"senderName":sender_name,"address":ip.to_string()}),
    );
    let accepted = tokio::time::timeout(Duration::from_secs(60), async {
        tokio::select! { biased;
            _ = receive(&mut ws) => {
                // No application messages are allowed until the receiver approves.
                // Reading here also releases the pending slot when the peer closes.
                "peer_closed"
            },
            result = rx => if result.unwrap_or(false) { "accepted" } else { "rejected" },
        }
    })
    .await
    .unwrap_or("timeout");
    server.pending.lock().await.remove(&connection);
    event(
        &server.events,
        "pair.closed",
        json!({"connectionId":connection,"reason":accepted}),
    );
    if accepted == "peer_closed" {
        return Ok(());
    }
    if accepted != "accepted" {
        send(&mut ws, &pair.error("PAIR_REJECTED")).await?;
        return Ok(());
    }
    let fingerprint = {
        use base64::Engine;
        hex::encode(Sha256::digest(
            base64::engine::general_purpose::STANDARD.decode(public)?,
        ))
    };
    let (tx, mut outgoing) = mpsc::channel::<Message>(32);
    let cancel = CancellationToken::new();
    {
        let mut connections = server.connections.lock().await;
        ensure!(
            !connections.contains_key(&owner),
            "DEVICE_ALREADY_CONNECTED"
        );
        connections.insert(
            owner,
            Connection {
                sender: tx,
                cancel: cancel.clone(),
            },
        );
    }
    let mut heartbeat = tokio::time::interval(Duration::from_secs(5));
    let mut last_seen = Instant::now();
    let run = async {
        send(&mut ws,&pair.reply("pair.accepted",json!({"receiverDeviceId":server.identity.id,"deviceToken":auth::random_token(),"receiverPublicKey":server.identity.public_key,"trust":"session_only"}))).await?;
        loop {
            tokio::select! {
                received = receive(&mut ws) => {
                    let mut request = received?;
                    last_seen = Instant::now();
                    if request.kind == "ping" {
                        send(&mut ws, &request.reply("pong", request.body.clone())).await?;
                        continue;
                    }
                    if request.kind == "pong" { continue; }
                    let response = handle_request(&server, owner, ip, &fingerprint, &mut request)
                        .await.unwrap_or_else(|e| request.error(&e.to_string()));
                    send(&mut ws, &response).await?;
                }
                message = outgoing.recv() => {
                    let Some(message) = message else { break; };
                    send(&mut ws, &message).await?;
                }
                _ = heartbeat.tick() => {
                    ensure!(last_seen.elapsed() < Duration::from_secs(15), "HEARTBEAT_TIMEOUT");
                    send(&mut ws, &Message::new("ping", json!({"nonce":Uuid::new_v4()}))).await?;
                }
            }
        }
        Ok(())
    };
    let outcome: anyhow::Result<()> = tokio::select! { biased;
        _ = cancel.cancelled() => Ok(()),
        result = run => result,
    };
    {
        let mut session = server.session.lock().await;
        if let Some(s) = session.as_mut()
            && s.owner == owner
        {
            s.stop();
            *session = None;
            event(
                &server.events,
                "session.closed",
                json!({"reason":"control_disconnected","requiresNewConsent":true}),
            );
        }
    }
    // Keep the owner reserved until the previous session has been removed.
    // Reversing this order lets a reconnect race the old connection's cleanup.
    server.connections.lock().await.remove(&owner);
    outcome
}
async fn handle_request(
    server: &Server,
    owner: Uuid,
    ip: IpAddr,
    fingerprint: &str,
    request: &mut Message,
) -> anyhow::Result<Message> {
    let mut guard = server.session.lock().await;
    if let Some(s) = guard.as_ref()
        && s.owner == owner
        && let Some(response) = s.cached(request.id, Instant::now())
    {
        return Ok(response);
    }
    if request.kind == "session.start" {
        ensure!(guard.is_none(), "BUSY");
        let mode = request.string("mode")?;
        let mut session = Session::new(owner, mode)?;
        session.rtc_recovery = mode == "mirror" && request.body["rtcRecovery"] == CAPABILITY;
        let mut response=request.reply("session.accepted",json!({"sessionId":session.id,"selectedProfile":Profile::conservative(server.variant=="legacy"),"audioPolicy":"explicit_capture_only"}));
        if session.rtc_recovery {
            response.body["rtcRecovery"] = json!(CAPABILITY);
        }
        response.session_id = Some(session.id);
        *guard = Some(session);
        guard
            .as_mut()
            .unwrap()
            .remember(request.id, response.clone(), Instant::now());
        event(
            &server.events,
            "session.started",
            json!({"sessionId":response.session_id,"mode":mode,"senderIp":ip.to_string(),"senderFingerprint":fingerprint,"rtcRecovery":response.body["rtcRecovery"]}),
        );
        return Ok(response);
    }
    if request.kind == "session.stop" && guard.is_none() {
        return Ok(request.reply("ack", json!({})));
    }
    let session = guard.as_mut().context("SESSION_EXPIRED")?;
    session.authorize(owner, request.session_id)?;
    match request.kind.as_str() {
        "rtc.offer" => {
            let previous = request
                .body
                .get("previousNegotiationId")
                .map(|v| {
                    v.as_str()
                        .context("INVALID_NEGOTIATION")
                        .and_then(|s| Ok(Uuid::parse_str(s)?))
                })
                .transpose()?;
            session.negotiate(Uuid::parse_str(request.string("negotiationId")?)?, previous)?;
        }
        "rtc.ice" => {
            ensure!(
                request.body["negotiationId"]
                    .as_str()
                    .and_then(|s| Uuid::parse_str(s).ok())
                    == session.negotiation,
                "STALE_NEGOTIATION"
            );
        }
        "file.load" => {
            ensure!(session.mode == "file", "INVALID_STATE");
            let url = url::Url::parse(request.string("url")?)?;
            ensure!(
                url.scheme() == "https"
                    && url
                        .host_str()
                        .and_then(|s| s.trim_matches(['[', ']']).parse::<IpAddr>().ok())
                        == Some(ip)
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.path().starts_with("/media/")
                    && url.query().is_none()
                    && url.fragment().is_none(),
                "UNSAFE_MEDIA_URL"
            );
            ensure!(request.string("mime")? == "video/mp4", "MEDIA_UNSUPPORTED");
            request.body["fingerprint"] = json!(fingerprint);
        }
        "playback.command" => {
            ensure!(session.mode == "file", "INVALID_STATE");
            match request.string("action")? {
                "play" | "pause" => {}
                "seek" => ensure!(
                    request.body["positionMs"].as_u64().is_some(),
                    "INVALID_POSITION"
                ),
                "volume" => ensure!(
                    request.body["value"]
                        .as_f64()
                        .is_some_and(|n| (0.0..=1.0).contains(&n)),
                    "INVALID_VOLUME"
                ),
                _ => anyhow::bail!("UNKNOWN_COMMAND"),
            }
        }
        "session.stop" => {}
        _ => anyhow::bail!("UNKNOWN_MESSAGE"),
    }
    event(&server.events, "message", serde_json::to_value(&request)?);
    let response = request.reply("ack", json!({}));
    session.remember(request.id, response.clone(), Instant::now());
    if request.kind == "session.stop" {
        session.stop();
        *guard = None;
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn saturated_signal_queue_cannot_prevent_local_stop() {
        let (events, _output) = std::sync::mpsc::sync_channel(16);
        let server = Server::new(Arc::new(Identity::create().unwrap()), "standard", events);
        let owner = Uuid::new_v4();
        let session = Session::new(owner, "file").unwrap();
        let id = session.id;
        *server.session.lock().await = Some(session);
        let (sender, _receiver) = mpsc::channel(1);
        sender
            .try_send(Message::new("statistics", json!({})))
            .unwrap();
        let cancel = CancellationToken::new();
        server.connections.lock().await.insert(
            owner,
            Connection {
                sender,
                cancel: cancel.clone(),
            },
        );
        let mut stop = Message::new("session.stop", json!({}));
        stop.session_id = Some(id);
        assert!(server.send(stop).await.is_err());
        assert!(server.session.lock().await.is_none());
        assert!(cancel.is_cancelled());
        server.stop().await;
    }
}
