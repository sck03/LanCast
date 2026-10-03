//! Strict gate for the project's single-program H.264 MPEG-TS output.
//! A new reader waits for PAT + PMT + actual SPS/PPS/IDR, not an encoder flag.
use anyhow::ensure;
pub const PACKET: usize = 188;
pub const MAX_WRITE: usize = 65_424;
pub fn validate(data: &[u8]) -> anyhow::Result<()> {
    ensure!(
        !data.is_empty() && data.len() <= MAX_WRITE && data.len().is_multiple_of(PACKET),
        "INVALID_TS_SIZE"
    );
    for p in data.as_chunks::<PACKET>().0 {
        payload(p)?;
    }
    Ok(())
}
fn payload(p: &[u8]) -> anyhow::Result<&[u8]> {
    ensure!(
        p.len() == PACKET && p[0] == 0x47 && p[1] & 0x80 == 0 && p[3] & 0xc0 == 0,
        "INVALID_TS_HEADER"
    );
    let control = (p[3] >> 4) & 3;
    ensure!(control != 0, "INVALID_TS_CONTROL");
    let start = if control & 2 != 0 {
        5 + p[4] as usize
    } else {
        4
    };
    ensure!(start <= PACKET, "INVALID_TS_ADAPTATION");
    Ok(if control & 1 == 0 { &[] } else { &p[start..] })
}
fn section(s: &[u8], table: u8) -> Option<&[u8]> {
    if s.len() < 8 || s[0] != table || s[5] & 1 == 0 {
        return None;
    }
    let size = 3 + (((s[1] & 15) as usize) << 8 | s[2] as usize);
    let s = s.get(..size)?;
    // ISO/IEC 13818-1 CRC32, including the transmitted CRC must have zero residue.
    let mut crc = u32::MAX;
    for b in s {
        crc ^= (*b as u32) << 24;
        for _ in 0..8 {
            crc = if crc & 0x80000000 != 0 {
                (crc << 1) ^ 0x04c11db7
            } else {
                crc << 1
            };
        }
    }
    (crc == 0).then_some(s)
}
#[derive(Default)]
struct SectionAssembler {
    bytes: Vec<u8>,
    continuity: Option<u8>,
}
impl SectionAssembler {
    fn push(&mut self, body: &[u8], start: bool, cc: u8) -> anyhow::Result<Option<Vec<u8>>> {
        if body.is_empty() {
            return Ok(None);
        }
        if start {
            self.bytes.clear();
            self.continuity = Some(cc);
            let offset = 1 + body[0] as usize;
            ensure!(offset <= body.len(), "INVALID_PSI_POINTER");
            self.bytes.extend_from_slice(&body[offset..]);
        } else if let Some(previous) = self.continuity {
            ensure!(cc == (previous + 1) & 15, "PSI_CONTINUITY_LOST");
            self.continuity = Some(cc);
            self.bytes.extend_from_slice(body);
        } else {
            return Ok(None);
        }
        if self.bytes.len() < 3 {
            return Ok(None);
        }
        let length = 3 + (((self.bytes[1] & 15) as usize) << 8 | self.bytes[2] as usize);
        ensure!((8..=1024).contains(&length), "INVALID_PSI_LENGTH");
        if self.bytes.len() < length {
            return Ok(None);
        }
        self.continuity = None;
        self.bytes.truncate(length);
        Ok(Some(std::mem::take(&mut self.bytes)))
    }
}
#[derive(Default)]
pub struct StartGate {
    pmt: Option<u16>,
    video: Option<u16>,
    sps: bool,
    pps: bool,
    idr: bool,
    tail: Vec<u8>,
    pending: Vec<u8>,
    ready: bool,
    pat_section: SectionAssembler,
    pmt_section: SectionAssembler,
    pat_packets: Vec<u8>,
    pes_header: Option<Vec<u8>>,
}
impl StartGate {
    pub fn push(&mut self, data: &[u8]) -> anyhow::Result<Option<Vec<u8>>> {
        validate(data)?;
        if self.ready {
            return Ok(Some(data.to_vec()));
        }
        for p in data.as_chunks::<PACKET>().0 {
            let pid = ((p[1] as u16 & 31) << 8) | p[2] as u16;
            let start = p[1] & 0x40 != 0;
            let mut body = payload(p)?;
            if pid == 0 && !body.is_empty() {
                if start {
                    self.pat_packets.clear();
                }
                ensure!(self.pat_packets.len() < 2048, "PAT_TOO_LARGE");
                self.pat_packets.extend_from_slice(p);
                if let Some(bytes) = self.pat_section.push(body, start, p[3] & 15)?
                    && let Some(pat) = section(&bytes, 0)
                {
                    let programs = pat.get(8..pat.len().saturating_sub(4)).unwrap_or_default();
                    for program in programs.as_chunks::<4>().0 {
                        if program[0] != 0 || program[1] != 0 {
                            if !self.idr {
                                self.pmt =
                                    Some(((program[2] as u16 & 31) << 8) | program[3] as u16);
                                self.video = None;
                                self.sps = false;
                                self.pps = false;
                                self.tail.clear();
                                self.pending.clear();
                                self.pending.extend_from_slice(&self.pat_packets);
                                self.pmt_section = SectionAssembler::default();
                            }
                            break;
                        }
                    }
                }
                continue;
            }
            if self.pmt.is_none() {
                continue;
            }
            ensure!(
                self.pending.len() + PACKET <= 8 * 1024 * 1024,
                "START_POINT_TOO_LARGE"
            );
            self.pending.extend_from_slice(p);
            if Some(pid) == self.pmt
                && let Some(bytes) = self.pmt_section.push(body, start, p[3] & 15)?
                && let Some(pmt) = section(&bytes, 2)
            {
                ensure!(pmt.len() >= 16, "INVALID_PMT");
                let mut offset = 12 + (((pmt[10] & 15) as usize) << 8 | pmt[11] as usize);
                while offset + 5 <= pmt.len() - 4 {
                    if pmt[offset] == 0x1b {
                        self.video =
                            Some(((pmt[offset + 1] as u16 & 31) << 8) | pmt[offset + 2] as u16);
                    }
                    offset +=
                        5 + (((pmt[offset + 3] & 15) as usize) << 8 | pmt[offset + 4] as usize);
                }
            }
            if Some(pid) != self.video || body.is_empty() {
                continue;
            }
            if start {
                self.pes_header = Some(Vec::new());
                self.tail.clear();
            }
            let joined;
            if let Some(header) = self.pes_header.as_mut() {
                header.extend_from_slice(body);
                if header.len() < 9 {
                    continue;
                }
                ensure!(
                    header[..3] == [0, 0, 1] && header[3] & 0xf0 == 0xe0,
                    "INVALID_VIDEO_PES"
                );
                let offset = 9 + header[8] as usize;
                if header.len() < offset {
                    continue;
                }
                joined = self.pes_header.take().unwrap();
                body = &joined[offset..];
            }
            self.tail.extend_from_slice(body);
            for nal in self.tail.windows(4) {
                if nal[..3] != [0, 0, 1] {
                    continue;
                }
                match nal[3] & 31 {
                    7 => self.sps = true,
                    8 if self.sps => self.pps = true,
                    5 if self.sps && self.pps => self.idr = true,
                    1 if !self.idr => {
                        self.sps = false;
                        self.pps = false;
                    }
                    _ => {}
                }
            }
            if self.tail.len() > 3 {
                self.tail.drain(..self.tail.len() - 3);
            }
        }
        if self.idr {
            self.ready = true;
            Ok(Some(std::mem::take(&mut self.pending)))
        } else {
            Ok(None)
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_invalid_framing_and_never_starts_on_a_flag() {
        for data in [vec![], vec![0; 188], vec![0x47; 189]] {
            assert!(validate(&data).is_err());
        }
        let mut packet = [0xff; 188];
        packet[..4].copy_from_slice(&[0x47, 0x41, 0, 0x10]);
        packet[4..9].copy_from_slice(&[0, 0, 1, 0x65, 0]);
        assert!(StartGate::default().push(&packet).unwrap().is_none());
    }
}
