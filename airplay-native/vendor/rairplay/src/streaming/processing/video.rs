//! Bounded mirror framing: idle streams may pause, but started frames must finish.
use std::{io, sync::Arc, time::Duration};

use bytes::{Buf, BytesMut};
use tokio::io::{AsyncRead, AsyncReadExt};

use super::{Encryption, build_video_cipher, memory::BytesHunk};
use crate::{
    playback::video::{PacketKind, VideoPacket, VideoStream},
    timing::Clock,
};

const FRAME_TIMEOUT: Duration = Duration::from_secs(5);

fn truncated(error: io::Error) -> io::Error {
    if error.kind() == io::ErrorKind::UnexpectedEof {
        io::Error::new(io::ErrorKind::InvalidData, "truncated video frame")
    } else {
        error
    }
}

async fn read_packet(
    reader: &mut (impl AsyncRead + Unpin),
    buffer: &mut BytesHunk,
    limit: u32,
    budget: Duration,
) -> io::Result<Option<([u8; 128], BytesMut)>> {
    let mut header = [0; 128];
    // A static screen may send nothing for an arbitrary time.
    if reader.read(&mut header[..1]).await? == 0 {
        return Ok(None);
    }
    tokio::time::timeout(budget, async {
        reader
            .read_exact(&mut header[1..])
            .await
            .map_err(truncated)?;
        let length = (&header[..]).get_u32_le();
        if length == 0 || length > limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "video frame limit",
            ));
        }
        let mut payload = buffer.allocate_buf(length as usize);
        reader.read_exact(&mut payload).await.map_err(truncated)?;
        Ok(Some((header, payload)))
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "video frame deadline"))?
}

pub async fn video_processor(
    mut reader: impl AsyncRead + Unpin,
    stream: &impl VideoStream,
    video_buf_size: u32,
    encryption: Encryption,
    clock: Arc<Clock>,
) -> io::Result<()> {
    // Grow with traffic rather than reserving the maximum frame before it arrives.
    let mut buffer = BytesHunk::new((video_buf_size as usize).min(64 * 1024));
    let mut cipher = build_video_cipher(&encryption)?;
    while let Some((header, mut payload)) =
        read_packet(&mut reader, &mut buffer, video_buf_size, FRAME_TIMEOUT).await?
    {
        let mut fields = &header[4..];
        let packet_type = fields.get_u16_le();
        let flags = fields.get_u16_le();
        let timestamp = fields.get_u64_le();
        let kind = match packet_type {
            1 if payload.len() >= 8 && &payload[4..8] == b"hvc1" => PacketKind::Hvc1(flags),
            1 => PacketKind::AvcC(flags),
            0 | 4096 => PacketKind::Payload,
            5 => PacketKind::Plist,
            other => PacketKind::Other(other),
        };
        if matches!(kind, PacketKind::Payload) {
            cipher.decrypt(header, &mut payload).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "video authentication failed")
            })?;
        }
        stream.on_data(VideoPacket {
            kind,
            timestamp,
            presentation_us: clock.local_us(timestamp),
            payload,
        });
        tokio::task::consume_budget().await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    #[test]
    fn unsupported_encryption_is_an_error_instead_of_a_task_panic() {
        for encryption in [
            Encryption::ChaCha { key: [0; 32] },
            Encryption::Legacy {
                key: [0; 16],
                iv: [0; 16],
                stream_connection_id: None,
            },
        ] {
            let Err(error) = build_video_cipher(&encryption) else {
                panic!("invalid key accepted")
            };
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        }
    }

    fn packet(data: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0; 128];
        bytes[..4].copy_from_slice(&(data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(data);
        bytes
    }

    #[tokio::test(start_paused = true)]
    async fn idle_resume_fragmentation_and_pipelining_preserve_frames() {
        let (mut reader, mut writer) = tokio::io::duplex(512);
        let send = tokio::spawn(async move {
            tokio::time::sleep(FRAME_TIMEOUT * 3).await;
            for frame in [packet(b"first"), packet(b"second")] {
                for byte in frame {
                    writer.write_all(&[byte]).await.unwrap();
                }
            }
        });
        let mut buffer = BytesHunk::new(32);
        for expected in [b"first".as_slice(), b"second"] {
            let (_, data) = read_packet(&mut reader, &mut buffer, 32, FRAME_TIMEOUT)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(&data[..], expected);
        }
        send.await.unwrap();
        assert!(
            read_packet(&mut reader, &mut buffer, 32, FRAME_TIMEOUT)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn partial_header_and_payload_expire_despite_trickling_bytes() {
        for prefix in [1, 128] {
            let (mut reader, mut writer) = tokio::io::duplex(512);
            let bytes = packet(&[1; 128]);
            writer.write_all(&bytes[..prefix]).await.unwrap();
            let send = tokio::spawn(async move {
                for byte in &bytes[prefix..] {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    if writer.write_all(&[*byte]).await.is_err() {
                        break;
                    }
                }
            });
            let begin = tokio::time::Instant::now();
            let error = read_packet(&mut reader, &mut BytesHunk::new(32), 128, FRAME_TIMEOUT)
                .await
                .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::TimedOut);
            assert_eq!(begin.elapsed(), FRAME_TIMEOUT);
            send.abort();
        }
    }

    #[tokio::test]
    async fn truncated_and_oversized_frames_are_rejected() {
        for bytes in [
            vec![1],
            packet(b"x")[..128].to_vec(),
            packet(&[1; 33]),
            packet(b""),
        ] {
            let error = read_packet(
                &mut bytes.as_slice(),
                &mut BytesHunk::new(32),
                32,
                FRAME_TIMEOUT,
            )
            .await
            .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        }
    }
}
