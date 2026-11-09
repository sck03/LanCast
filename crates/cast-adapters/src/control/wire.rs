//! Bounded WSS framing shared by both control roles.
use crate::protocol::{MAX_FRAME, MAX_MESSAGE, Message, json_text};
use anyhow::Context;
use futures_util::{SinkExt, StreamExt};
use std::time::Duration;
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{Message as Frame, protocol::WebSocketConfig},
};

const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

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
    let frame = Frame::Text(json_text(message, MAX_MESSAGE)?.into());
    tokio::time::timeout(WRITE_TIMEOUT, ws.send(frame)).await??;
    Ok(())
}
pub(super) async fn receive<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    ws: &mut WebSocketStream<S>,
) -> anyhow::Result<Message> {
    loop {
        // Buffered control frames need no socket I/O and can otherwise prevent
        // the owner from polling cancellation, deadlines or outgoing signals.
        // Yield before consuming a frame so cancelling receive never loses text.
        tokio::task::consume_budget().await;
        match ws.next().await.context("DISCONNECTED")?? {
            Frame::Text(text) => return Message::parse(&text),
            // tungstenite already queues the matching Pong. Flush that response
            // with the same write deadline instead of replacing it manually.
            Frame::Ping(_) => tokio::time::timeout(WRITE_TIMEOUT, ws.flush()).await??,
            Frame::Pong(_) => {}
            _ => anyhow::bail!("DISCONNECTED"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_tungstenite::tungstenite::protocol::{
        Role,
        frame::{
            Frame as WireFrame,
            coding::{Data, OpCode},
        },
    };

    fn encode_frames(frames: impl IntoIterator<Item = WireFrame>) -> Vec<u8> {
        let mut bytes = Vec::new();
        for frame in frames {
            frame.format(&mut bytes).unwrap();
        }
        bytes
    }

    #[tokio::test]
    async fn buffered_control_frames_yield_to_other_work() {
        let (client, server) = tokio::io::duplex(16 * 1024);
        let mut client =
            WebSocketStream::from_raw_socket(client, Role::Client, Some(wire_config())).await;
        let mut server =
            WebSocketStream::from_raw_socket(server, Role::Server, Some(wire_config())).await;
        let message = Message::new("ping", serde_json::json!({}));
        // All frames fit into one read. Transport I/O alone cannot provide
        // cooperative scheduling while the WebSocket buffer stays readable.
        tokio::task::unconstrained(async {
            for _ in 0..1024 {
                server.feed(Frame::Pong(Vec::new().into())).await.unwrap();
            }
            send(&mut server, &message).await.unwrap();
        })
        .await;
        tokio::task::yield_now().await;
        assert!(
            futures_util::FutureExt::now_or_never(receive(&mut client)).is_none(),
            "one receive monopolized the task until the entire control burst was drained"
        );
        // Cancelling a yielded read must retain the following application frame.
        assert_eq!(receive(&mut client).await.unwrap().id, message.id);
    }

    #[tokio::test]
    async fn cancelled_pong_flush_preserves_control_and_application_frames() {
        let message = Message::new("ping", serde_json::json!({}));
        let first = vec![7; 125];
        let second = vec![9];
        let buffered = encode_frames([
            WireFrame::ping(first.clone()),
            WireFrame::ping(second.clone()),
            WireFrame::message(
                serde_json::to_vec(&message).unwrap(),
                OpCode::Data(Data::Text),
                true,
            ),
        ]);
        // The first Pong cannot fit until the peer starts reading. Cancelling
        // receive here models the owner's timer/outgoing branch winning select!.
        let (client, server) = tokio::io::duplex(16);
        let mut client = WebSocketStream::from_partially_read(
            client,
            buffered,
            Role::Client,
            Some(wire_config()),
        )
        .await;
        let mut server =
            WebSocketStream::from_raw_socket(server, Role::Server, Some(wire_config())).await;
        assert!(futures_util::FutureExt::now_or_never(receive(&mut client)).is_none());
        let (received, ()) = tokio::join!(receive(&mut client), async {
            for payload in [first, second] {
                assert_eq!(
                    server.next().await.unwrap().unwrap(),
                    Frame::Pong(payload.into())
                );
            }
        });
        assert_eq!(received.unwrap().id, message.id);
        assert!(
            futures_util::FutureExt::now_or_never(server.next()).is_none(),
            "a cancelled Pong write caused a duplicate reply to the next Ping"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn stalled_automatic_pong_flush_has_a_write_deadline() {
        let (client, _unresponsive_peer) = tokio::io::duplex(16);
        let mut client = WebSocketStream::from_partially_read(
            client,
            encode_frames([WireFrame::ping(vec![7; 125])]),
            Role::Client,
            Some(wire_config()),
        )
        .await;
        let started = tokio::time::Instant::now();
        let error = receive(&mut client).await.unwrap_err();
        assert!(error.is::<tokio::time::error::Elapsed>());
        assert_eq!(started.elapsed(), Duration::from_secs(5));
    }

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
