//! LanCast NTP clock and RTP anchors. No arrival-time substitute for presentation time.
use std::{
    io,
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;

const EPOCH_US: i64 = 2_208_988_800_000_000;
#[derive(Default, Debug)]
struct Estimate {
    offset: Option<i64>,
    best_delay: i64,
    samples: u32,
    measured: Option<std::time::Instant>,
}
#[derive(Default, Debug)]
pub struct Clock {
    estimate: Mutex<Estimate>,
}

pub fn fixed_to_us(stamp: u64) -> i64 {
    ((stamp >> 32) * 1_000_000 + (((stamp & 0xffff_ffff) * 1_000_000) >> 32)) as i64
}
pub fn now_fixed() -> u64 {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let sec = elapsed.as_secs() + 2_208_988_800;
    (sec << 32) | ((u64::from(elapsed.subsec_nanos()) << 32) / 1_000_000_000)
}
impl Clock {
    pub fn measure(&self, t1: u64, t2: u64, t3: u64, t4: u64) -> bool {
        let [a, b, c, d] = [t1, t2, t3, t4].map(fixed_to_us);
        let delay = (d - a) - (c - b);
        if !(0..=1_000_000).contains(&delay) || d < a || c < b {
            return false;
        }
        let offset = ((a - b) + (d - c)) / 2;
        let mut e = self.estimate.lock().unwrap();
        // Refresh the min-delay window rather than trusting an old measurement forever.
        if e.offset.is_none() || e.samples.is_multiple_of(8) || delay <= e.best_delay {
            e.offset = Some(offset);
            e.best_delay = delay;
        }
        e.samples = e.samples.wrapping_add(1);
        e.measured = Some(std::time::Instant::now());
        true
    }
    pub fn local_us(&self, remote: u64) -> Option<i64> {
        let estimate = self.estimate.lock().unwrap();
        if estimate.measured?.elapsed() > Duration::from_secs(6) {
            return None;
        }
        estimate
            .offset
            .map(|offset| fixed_to_us(remote) + offset - EPOCH_US)
    }
}

#[derive(Default, Debug)]
pub struct AudioClock {
    anchor: Mutex<Option<(u32, u64)>>,
}
impl AudioClock {
    pub fn sync(&self, packet: &[u8]) {
        if packet.len() >= 20 && packet[1] & 0x7f == 0x54 {
            let rtp = u32::from_be_bytes(packet[4..8].try_into().unwrap());
            let ntp = u64::from_be_bytes(packet[8..16].try_into().unwrap());
            *self.anchor.lock().unwrap() = Some((rtp, ntp));
        }
    }
    pub fn local_us(&self, clock: &Clock, rtp: u32, rate: u32) -> Option<i64> {
        if rate == 0 {
            return None;
        }
        let (base, ntp) = (*self.anchor.lock().unwrap())?;
        let delta = i64::from(rtp.wrapping_sub(base) as i32);
        if delta.abs() > i64::from(rate) * 10 {
            return None;
        }
        Some(clock.local_us(ntp)? + delta * 1_000_000 / i64::from(rate))
    }
}

/// Returns the actual timing port; the task exits with the control connection.
pub async fn start(
    bind: IpAddr,
    remote: SocketAddr,
    clock: Arc<Clock>,
    cancel: CancellationToken,
) -> io::Result<u16> {
    if remote.port() == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "missing timing port",
        ));
    }
    let socket = UdpSocket::bind(SocketAddr::new(bind, 0)).await?;
    let port = socket.local_addr()?.port();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        let mut pending = 0u64;
        let mut sequence = 0u16;
        let mut buf = [0u8; 64];
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = interval.tick() => {
                    let mut request = [0u8;32]; request[0]=0x80; request[1]=0xd2;
                    sequence = sequence.wrapping_add(1); request[2..4].copy_from_slice(&sequence.to_be_bytes());
                    pending = now_fixed(); request[24..32].copy_from_slice(&pending.to_be_bytes());
                    if socket.send_to(&request,remote).await.is_err() { break; }
                },
                received = socket.recv_from(&mut buf) => {
                    let Ok((32, peer)) = received else { continue; };
                    if peer != remote { continue; }
                    let received_at = now_fixed();
                    match buf[1] & 0x7f {
                        0x53 => {
                            let origin = u64::from_be_bytes(buf[8..16].try_into().unwrap());
                            if pending == 0 || origin != pending { continue; }
                            let t2 = u64::from_be_bytes(buf[16..24].try_into().unwrap());
                            let t3 = u64::from_be_bytes(buf[24..32].try_into().unwrap());
                            clock.measure(origin,t2,t3,received_at); pending=0;
                        },
                        0x52 => {
                            let mut reply=[0u8;32]; reply[0]=0x80; reply[1]=0xd3;
                            reply[2..4].copy_from_slice(&buf[2..4]); reply[8..16].copy_from_slice(&buf[24..32]);
                            reply[16..24].copy_from_slice(&received_at.to_be_bytes()); reply[24..32].copy_from_slice(&now_fixed().to_be_bytes());
                            let _ = socket.send_to(&reply,remote).await;
                        },
                        _ => {},
                    }
                }
            }
        }
    });
    Ok(port)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn f(seconds: u64) -> u64 {
        seconds << 32
    }
    #[test]
    fn clock_uses_four_timestamps_and_audio_wraps_safely() {
        let clock = Clock::default();
        assert!(clock.local_us(f(100)).is_none());
        assert!(clock.measure(f(100), f(110), f(110), f(100)));
        assert_eq!(clock.local_us(f(110)), Some(100_000_000 - EPOCH_US));
        let audio = AudioClock::default();
        let mut sync = [0u8; 20];
        sync[1] = 0xd4;
        sync[4..8].copy_from_slice(&(u32::MAX - 479).to_be_bytes());
        sync[8..16].copy_from_slice(&f(110).to_be_bytes());
        audio.sync(&sync);
        assert_eq!(
            audio.local_us(&clock, 0, 48000),
            Some(100_010_000 - EPOCH_US)
        );
        assert!(!clock.measure(f(100), f(110), f(112), f(101)));
    }
}
