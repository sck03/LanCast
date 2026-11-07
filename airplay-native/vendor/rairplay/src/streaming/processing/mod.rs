use std::{io, net::IpAddr};

use tokio::{
    io::AsyncReadExt,
    net::{TcpListener, UdpSocket},
};
use tracing::Instrument;

use super::EncryptionMaterial;
use crate::{
    crypto::{AesIv128, AesKey128, ChaCha20Poly1305Key},
    pairing::SessionKey,
    playback::audio::{AudioPacket, AudioStream},
};

mod crypto;
mod memory;
mod video;
pub use video::video_processor;

#[derive(Debug)]
pub enum Encryption {
    ChaCha {
        key: ChaCha20Poly1305Key,
    },
    HomeKit {
        key: SessionKey,
        stream_connection_id: u64,
    },
    Legacy {
        key: AesKey128,
        iv: AesIv128,
        stream_connection_id: Option<u64>,
    },
}

impl TryFrom<EncryptionMaterial> for Encryption {
    type Error = io::Error;

    fn try_from(value: EncryptionMaterial) -> Result<Self, Self::Error> {
        if let Some(key) = value.chacha_key {
            Ok(Encryption::ChaCha { key })
        } else if let Some(key) = value.aeskey
            && let Some(iv) = value.aesiv
        {
            Ok(Encryption::Legacy {
                key,
                iv,
                stream_connection_id: value.stream_connection_id,
            })
        } else if let Some(key) = value.session_key
            && key.upgrade_channel
            && let Some(stream_connection_id) = value.stream_connection_id
        {
            Ok(Encryption::HomeKit {
                key,
                stream_connection_id,
            })
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "no encryption key passed",
            ))
        }
    }
}

#[tracing::instrument(level = "DEBUG")]
pub async fn event_processor(listener: TcpListener) {
    let mut buf = [0; 16 * 1024];
    while let Ok((mut stream, remote_addr)) = listener.accept().await {
        while let Ok(len @ 1..) = stream.read(&mut buf).await {
            tracing::trace!(%len, %remote_addr, "event data");
        }
    }
}

#[tracing::instrument(level = "DEBUG", skip(stream))]
#[allow(clippy::too_many_arguments)] // Explicit transport, codec and negotiated clock inputs.
pub async fn audio_realtime_processor(
    expected_remote_addr: IpAddr,
    socket: UdpSocket,
    stream: &impl AudioStream,
    audio_buf_size: u32,
    encryption: Encryption,
    clock: std::sync::Arc<crate::timing::Clock>,
    audio_clock: std::sync::Arc<crate::timing::AudioClock>,
    sample_rate: u32,
) -> io::Result<()> {
    let mut pkt_buf = [0u8; 16 * 1024];
    let mut audio_buf = memory::BytesHunk::new((audio_buf_size as usize).min(pkt_buf.len()));
    let cipher = build_audio_cipher(&encryption);

    loop {
        async {
            let (pkt_len, remote_addr) = socket.recv_from(&mut pkt_buf).await?;

            // Filter out unexpected addresses
            if expected_remote_addr == remote_addr.ip() {
                if pkt_len < AudioPacket::HEADER_LEN || pkt_len > audio_buf_size as usize {
                    tracing::warn!(%pkt_len, "malformed packet");
                } else {
                    let mut rtp = audio_buf.allocate_buf(pkt_len);
                    rtp.copy_from_slice(&pkt_buf[..pkt_len]);
                    tracing::trace!(%pkt_len, "packet read");

                    if rtp[0] != 0x80 {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "unsupported RTP header",
                        ));
                    }
                    cipher.decrypt(&mut rtp).map_err(|_| {
                        io::Error::new(io::ErrorKind::InvalidData, "audio authentication failed")
                    })?;
                    let stamp = u32::from_be_bytes(rtp[4..8].try_into().unwrap());
                    let presentation_us = audio_clock.local_us(&clock, stamp, sample_rate);
                    stream.on_data(AudioPacket {
                        rtp,
                        presentation_us,
                    });
                    tokio::task::consume_budget().await;
                }
            } else {
                tracing::debug!(%remote_addr, "skip invalid connection");
            }

            io::Result::Ok(())
        }
        .instrument(tracing::debug_span!("packet.realtime"))
        .await?;
    }
}

#[tracing::instrument(level = "DEBUG", err)]
pub async fn control_processor(
    expected_remote_addr: IpAddr,
    socket: UdpSocket,
    clock: std::sync::Arc<crate::timing::AudioClock>,
) -> io::Result<()> {
    const BUF_SIZE: usize = 16 * 1024;

    let mut buf = [0u8; BUF_SIZE];
    loop {
        let (len, peer) = socket.recv_from(&mut buf).await?;
        if peer.ip() == expected_remote_addr {
            clock.sync(&buf[..len]);
        }
    }
}

fn build_audio_cipher(encryption: &Encryption) -> Box<dyn crypto::AudioCipher + Send + Sync> {
    match encryption {
        Encryption::ChaCha { key } => Box::new(crypto::ChachaAudioCipher::from_key(*key)),
        Encryption::HomeKit {
            key,
            stream_connection_id,
        } => Box::new(crypto::ChachaAudioCipher::from_secret_and_id(
            &key.key_material,
            *stream_connection_id,
        )),
        Encryption::Legacy { key, iv, .. } => Box::new(crypto::AesAudioCipher::new(*key, *iv)),
    }
}

fn build_video_cipher(
    encryption: &Encryption,
) -> io::Result<Box<dyn crypto::VideoCipher + Send + Sync>> {
    Ok(match encryption {
        Encryption::HomeKit {
            key,
            stream_connection_id,
        } => Box::new(crypto::ChachaVideoCipher::from_secret_and_id(
            &key.key_material,
            *stream_connection_id,
        )),
        Encryption::Legacy {
            key,
            stream_connection_id: Some(stream_connection_id),
            ..
        } => Box::new(crypto::AesVideoCipher::from_key_and_id(
            *key,
            *stream_connection_id,
        )),
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "unsupported video encryption",
            ));
        }
    })
}
