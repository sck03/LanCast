//! Bounded WSS framing shared by both control roles.
use crate::protocol::{MAX_FRAME, MAX_MESSAGE, Message, json_text};
use anyhow::Context;
use futures_util::{SinkExt, StreamExt};
use std::time::Duration;
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{Message as Frame, protocol::WebSocketConfig},
};

pub(super) fn wire_config() -> WebSocketConfig {
    WebSocketConfig::default()
        // Control traffic is mostly small signals. Larger SDP still grows up
        // to MAX_FRAME; avoid reserving the default 128 KiB for every reader.
        .read_buffer_size(16 * 1024)
        .max_message_size(Some(MAX_FRAME))
        .max_frame_size(Some(MAX_FRAME))
        .max_write_buffer_size(512 * 1024)
}

pub(super) async fn send<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    ws: &mut WebSocketStream<S>,
    message: &Message,
) -> anyhow::Result<()> {
    send_frame(ws, Frame::Text(json_text(message, MAX_MESSAGE)?.into())).await
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_tungstenite::tungstenite::protocol::Role;

    #[tokio::test]
    async fn oversized_outgoing_message_is_rejected_before_any_socket_write() {
        use tokio::io::AsyncReadExt;
        let (client, mut peer) = tokio::io::duplex(1024);
        let mut client =
            WebSocketStream::from_raw_socket(client, Role::Client, Some(wire_config())).await;
        let message = Message::new(
            "session.state",
            serde_json::json!({"padding":"x".repeat(MAX_MESSAGE)}),
        );
        assert!(
            send(&mut client, &message)
                .await
                .unwrap_err()
                .to_string()
                .contains("FRAME_TOO_LARGE")
        );
        assert!(futures_util::FutureExt::now_or_never(peer.read(&mut [0])).is_none());
    }

    #[tokio::test]
    async fn large_sdp_crosses_small_transport_and_read_buffers() {
        let (client, server) = tokio::io::duplex(1024);
        let mut client =
            WebSocketStream::from_raw_socket(client, Role::Client, Some(wire_config())).await;
        let mut server =
            WebSocketStream::from_raw_socket(server, Role::Server, Some(wire_config())).await;
        let message = Message::new(
            "rtc.offer",
            serde_json::json!({
                "sdp": "x".repeat(crate::protocol::MAX_SDP),
                "negotiationId": uuid::Uuid::new_v4(),
            }),
        );
        let (sent, received) = tokio::join!(send(&mut client, &message), receive(&mut server));
        sent.unwrap();
        let received = received.unwrap();
        assert_eq!(received.id, message.id);
        assert_eq!(received.body, message.body);
    }
}
