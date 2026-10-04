//! Bounded WSS framing shared by both control roles.
use crate::protocol::{MAX_FRAME, Message};
use anyhow::Context;
use futures_util::{SinkExt, StreamExt};
use std::time::Duration;
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{Message as Frame, protocol::WebSocketConfig},
};

pub(super) fn wire_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(MAX_FRAME))
        .max_frame_size(Some(MAX_FRAME))
        .max_write_buffer_size(512 * 1024)
}
pub(super) async fn send<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    ws: &mut WebSocketStream<S>,
    message: &Message,
) -> anyhow::Result<()> {
    send_frame(ws, Frame::Text(serde_json::to_string(message)?.into())).await
}
async fn send_frame<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    ws: &mut WebSocketStream<S>,
    frame: Frame,
) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(5), ws.send(frame)).await??;
    Ok(())
}
pub(super) async fn receive<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    ws: &mut WebSocketStream<S>,
) -> anyhow::Result<Message> {
    loop {
        match ws.next().await.context("DISCONNECTED")?? {
            Frame::Text(text) => return Message::parse(&text),
            Frame::Ping(data) => send_frame(ws, Frame::Pong(data)).await?,
            Frame::Pong(_) => {}
            _ => anyhow::bail!("DISCONNECTED"),
        }
    }
}
