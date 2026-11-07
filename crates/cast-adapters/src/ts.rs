//! Strict gate for the project's single-program H.264 MPEG-TS output.
//! A new reader waits for PAT + PMT + actual SPS/PPS/IDR, not an encoder flag.
use anyhow::ensure;
pub const PACKET: usize = 188;
pub const MAX_WRITE: usize = 65_424;
const MAX_START: usize = 8 * 1024 * 1024;
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
    fn reset(&mut self) {
        self.bytes.clear();
        self.continuity = None;
    }
    fn push(&mut self, body: &[u8], start: bool, cc: u8) -> anyhow::Result<Option<&[u8]>> {
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
        Ok(Some(&self.bytes))
    }
}
#[derive(Default)]
struct VideoStart {
    continuity: Option<u8>,
    header: Option<Vec<u8>>,
    prefix: u32,
    sps: bool,
    pps: bool,
    idr: bool,
}
fn pes_offset(header: &[u8]) -> anyhow::Result<Option<usize>> {
    if header.len() < 9 {
        return Ok(None);
    }
    ensure!(
        header[..3] == [0, 0, 1] && header[3] & 0xf0 == 0xe0,
        "INVALID_VIDEO_PES"
    );
    let offset = 9 + header[8] as usize;
    Ok((header.len() >= offset).then_some(offset))
}
impl VideoStart {
    fn push(&mut self, body: &[u8], start: bool, cc: u8) -> anyhow::Result<()> {
        let mut body = body;
        if body.is_empty() {
            return Ok(());
        }
        if let Some(previous) = self.continuity {
            ensure!(cc == (previous + 1) & 15, "VIDEO_CONTINUITY_LOST");
        } else if !start {
            // Joining in the middle of a PES cannot produce a decodable prefix.
            return Ok(());
        }
        self.continuity = Some(cc);
        if start {
            self.prefix = u32::MAX;
            // Most PES headers fit in the first TS packet: scan its payload in
            // place and allocate scratch space only for genuinely split headers.
            self.header = None;
            if let Some(offset) = pes_offset(body)? {
                body = &body[offset..];
            } else {
                self.header = Some(body.to_vec());
                return Ok(());
            }
        }
        let joined;
        if let Some(header) = self.header.as_mut() {
            header.extend_from_slice(body);
            let Some(offset) = pes_offset(header)? else {
                return Ok(());
            };
            joined = self.header.take().unwrap();
            body = &joined[offset..];
        }
        // A rolling prefix handles split Annex B start codes without a per-packet
        // scratch allocation or moving the previous packet's tail.
        for byte in body {
            self.prefix = (self.prefix << 8) | u32::from(*byte);
            if self.prefix & 0xffff_ff00 != 0x0000_0100 {
                continue;
            }
            match byte & 31 {
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
        Ok(())
    }
}
#[derive(Clone, Copy)]
struct Program {
    number: u16,
    pmt: u16,
}
fn pat_program(pat: &[u8]) -> anyhow::Result<Option<Program>> {
    ensure!(pat.len() >= 12, "INVALID_PAT");
    let (programs, remainder) = pat[8..pat.len() - 4].as_chunks::<4>();
    ensure!(remainder.is_empty(), "INVALID_PAT_PROGRAMS");
    Ok(programs.iter().find_map(|p| {
        let number = u16::from_be_bytes([p[0], p[1]]);
        (number != 0).then_some(Program {
            number,
            pmt: (u16::from(p[2] & 31) << 8) | u16::from(p[3]),
        })
    }))
}
#[derive(Default)]
pub struct StartGate {
    program: Option<Program>,
    video: Option<u16>,
    video_start: VideoStart,
    pending: Vec<u8>,
    ready: bool,
    pat_section: SectionAssembler,
    pmt_section: SectionAssembler,
    pat_packets: Vec<u8>,
    pat_length: usize,
    pmt_packets: Vec<u8>,
    program_map: Vec<u8>,
}
fn video_pid(pmt: &[u8]) -> anyhow::Result<Option<u16>> {
    ensure!(pmt.len() >= 16, "INVALID_PMT");
    let end = pmt.len() - 4;
    let mut offset = 12 + (((pmt[10] & 15) as usize) << 8 | pmt[11] as usize);
    ensure!(offset <= end, "INVALID_PMT_DESCRIPTORS");
    let mut video = None;
    while offset < end {
        ensure!(offset + 5 <= end, "INVALID_PMT_STREAM");
        let next = offset + 5 + (((pmt[offset + 3] & 15) as usize) << 8 | pmt[offset + 4] as usize);
        ensure!(next <= end, "INVALID_PMT_DESCRIPTORS");
        if pmt[offset] == 0x1b {
            video = Some(((pmt[offset + 1] as u16 & 31) << 8) | pmt[offset + 2] as u16);
        }
        offset = next;
    }
    Ok(video)
}
impl StartGate {
    pub fn push(&mut self, data: &[u8]) -> anyhow::Result<Option<Vec<u8>>> {
        validate(data)?;
        if self.ready {
            return Ok(Some(data.to_vec()));
        }
        for (index, p) in data.as_chunks::<PACKET>().0.iter().enumerate() {
            let pid = ((p[1] as u16 & 31) << 8) | p[2] as u16;
            let start = p[1] & 0x40 != 0;
            let body = payload(p)?;
            if pid == 0 && !body.is_empty() {
                if start {
                    self.pat_packets.clear();
                }
                ensure!(self.pat_packets.len() < 2048, "PAT_TOO_LARGE");
                self.pat_packets.extend_from_slice(p);
                if let Some(bytes) = self.pat_section.push(body, start, p[3] & 15)?
                    && let Some(pat) = section(bytes, 0)
                {
                    self.program = pat_program(pat)?;
                    self.video = None;
                    self.video_start = VideoStart::default();
                    self.pending.clear();
                    self.pending.extend_from_slice(&self.pat_packets);
                    self.pat_length = self.pending.len();
                    self.pmt_section.reset();
                    self.pmt_packets.clear();
                    self.program_map.clear();
                }
                continue;
            }
            let Some(program) = self.program else {
                continue;
            };
            if Some(pid) == self.video && !start && self.video_start.continuity.is_none() {
                continue;
            }
            ensure!(
                self.pending.len() + PACKET <= MAX_START,
                "START_POINT_TOO_LARGE"
            );
            self.pending.extend_from_slice(p);
            if pid == program.pmt && !body.is_empty() {
                if start {
                    self.pmt_packets.clear();
                }
                ensure!(self.pmt_packets.len() < 2048, "PMT_TOO_LARGE");
                self.pmt_packets.extend_from_slice(p);
                if let Some(bytes) = self.pmt_section.push(body, start, p[3] & 15)?
                    && let Some(pmt) = section(bytes, 2)
                    && u16::from_be_bytes([pmt[3], pmt[4]]) == program.number
                    && pmt != self.program_map
                {
                    self.video = video_pid(pmt)?;
                    self.video_start = VideoStart::default();
                    self.program_map.clear();
                    self.program_map.extend_from_slice(pmt);
                    // Publish only the current program and its own video start.
                    // Keep every packet when the PMT spans multiple TS packets.
                    self.pending.truncate(self.pat_length);
                    self.pending.extend_from_slice(&self.pmt_packets);
                }
            }
            if Some(pid) == self.video {
                self.video_start.push(body, start, p[3] & 15)?;
            }
            if self.video_start.idr {
                // The first valid start ends parsing immediately, regardless of
                // write boundaries. Later PAT/PMT/PES packets belong to playback.
                let tail = &data[(index + 1) * PACKET..];
                ensure!(
                    self.pending.len() + tail.len() <= MAX_START,
                    "START_POINT_TOO_LARGE"
                );
                self.pending.extend_from_slice(tail);
                let output = std::mem::take(&mut self.pending);
                // Drop startup allocations even when a caller retains this gate.
                *self = Self {
                    ready: true,
                    ..Self::default()
                };
                return Ok(Some(output));
            }
        }
        Ok(None)
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn packet(pid: u16, start: bool, cc: u8, body: &[u8]) -> [u8; PACKET] {
        assert!(body.len() <= 182);
        let mut packet = [0xff; PACKET];
        packet[..4].copy_from_slice(&[
            0x47,
            (pid >> 8) as u8 | if start { 0x40 } else { 0 },
            pid as u8,
            0x30 | cc,
        ]);
        packet[4] = (183 - body.len()) as u8;
        packet[5] = 0;
        packet[PACKET - body.len()..].copy_from_slice(body);
        packet
    }

    fn psi(pid: u16, section: &[u8]) -> [u8; PACKET] {
        // Independently append the MPEG-2 CRC to a complete single-program section.
        let mut crc = u32::MAX;
        for byte in section {
            crc ^= u32::from(*byte) << 24;
            for _ in 0..8 {
                crc = (crc << 1)
                    ^ if crc & 0x8000_0000 != 0 {
                        0x04c1_1db7
                    } else {
                        0
                    };
            }
        }
        let mut body = vec![0];
        body.extend_from_slice(section);
        body.extend_from_slice(&crc.to_be_bytes());
        packet(pid, true, 0, &body)
    }

    fn tables(gate: &mut StartGate) {
        let pat = psi(0, &[0, 0xb0, 13, 0, 1, 0xc1, 0, 0, 0, 1, 0xf0, 0]);
        let pmt = psi(
            0x1000,
            &[
                2, 0xb0, 18, 0, 1, 0xc1, 0, 0, 0xe1, 0, 0xf0, 0, 0x1b, 0xe1, 0, 0xf0, 0,
            ],
        );
        assert!(gate.push(&pat).unwrap().is_none());
        assert!(gate.push(&pmt).unwrap().is_none());
    }

    const PES: &[u8] = &[0, 0, 1, 0xe0, 0, 0, 0x80, 0, 0];
    const KEYFRAME: &[u8] = &[0, 0, 1, 0x67, 0, 0, 1, 0x68, 0, 0, 1, 0x65];

    fn program_map(video: u8, version: u8, codec: u8) -> [u8; PACKET] {
        psi(
            0x1000,
            &[
                2,
                0xb0,
                18,
                0,
                1,
                0xc1 | (version << 1),
                0,
                0,
                0xe1,
                video,
                0xf0,
                0,
                codec,
                0xe1,
                video,
                0xf0,
                0,
            ],
        )
    }

    #[test]
    fn program_changes_after_the_first_idr_are_independent_of_write_boundaries() {
        let first_frame = packet(0x100, true, 0, &[PES, KEYFRAME].concat());
        for pmt_pid in [0x1000, 0x1001] {
            let pat = psi(
                0,
                &[0, 0xb0, 13, 0, 1, 0xc3, 0, 0, 0, 2, 0xf0, pmt_pid as u8],
            );
            let pmt = psi(
                pmt_pid,
                &[
                    2, 0xb0, 18, 0, 2, 0xc1, 0, 0, 0xe1, 1, 0xf0, 0, 0x1b, 0xe1, 1, 0xf0, 0,
                ],
            );
            // The next program has not produced a keyframe yet. The first
            // program's valid start must still be published, with all later TS intact.
            let input = [first_frame, pat, pmt].concat();
            let mut reference = StartGate::default();
            tables(&mut reference);
            let expected: Vec<u8> = input
                .as_chunks::<PACKET>()
                .0
                .iter()
                .flat_map(|p| reference.push(p).unwrap().unwrap())
                .collect();
            for cut in 1..=3 {
                let mut gate = StartGate::default();
                tables(&mut gate);
                let actual: Vec<u8> = input
                    .chunks(cut * PACKET)
                    .flat_map(|part| gate.push(part).unwrap().unwrap_or_default())
                    .collect();
                assert_eq!(actual, expected, "PMT {pmt_pid:#x}, {cut} packets/write");
            }
        }
    }

    #[test]
    fn pat_without_a_program_cannot_reuse_previous_video() {
        for section in [
            vec![0, 0xb0, 9, 0, 1, 0xc3, 0, 0],
            vec![0, 0xb0, 13, 0, 1, 0xc3, 0, 0, 0, 0, 0xe0, 0x10],
        ] {
            let mut gate = StartGate::default();
            tables(&mut gate);
            gate.push(&packet(0x100, true, 0, &[PES, &KEYFRAME[..8]].concat()))
                .unwrap();
            gate.push(&psi(0, &section)).unwrap();
            assert!(
                gate.push(&packet(0x100, true, 1, &[PES, KEYFRAME].concat()))
                    .unwrap()
                    .is_none(),
                "removed program still opened the gate"
            );
            tables(&mut gate);
            assert!(
                gate.push(&packet(0x100, true, 7, &[PES, KEYFRAME].concat()))
                    .unwrap()
                    .is_some()
            );
        }
    }

    #[test]
    fn pmt_must_belong_to_the_program_selected_by_pat() {
        let mut gate = StartGate::default();
        let pat = psi(0, &[0, 0xb0, 13, 0, 1, 0xc1, 0, 0, 0, 2, 0xf0, 0]);
        gate.push(&pat).unwrap();
        gate.push(&program_map(0, 0, 0x1b)).unwrap(); // Program 1 on the same PMT PID.
        assert!(
            gate.push(&packet(0x100, true, 0, &[PES, KEYFRAME].concat()))
                .unwrap()
                .is_none(),
            "accepted another program's PMT"
        );
        let pmt = psi(
            0x1000,
            &[
                2, 0xb0, 18, 0, 2, 0xc1, 0, 0, 0xe1, 0, 0xf0, 0, 0x1b, 0xe1, 0, 0xf0, 0,
            ],
        );
        gate.push(&pmt).unwrap();
        let keyframe = packet(0x100, true, 9, &[PES, KEYFRAME].concat());
        assert_eq!(
            gate.push(&keyframe).unwrap().unwrap(),
            [pat, pmt, keyframe].concat()
        );
    }

    #[test]
    fn malformed_pat_cannot_select_a_partial_program_entry() {
        for section in [
            vec![0, 0xb0, 8, 0, 1, 0xc1, 0],
            vec![0, 0xb0, 14, 0, 1, 0xc1, 0, 0, 0, 1, 0xf0, 0, 9],
        ] {
            assert!(StartGate::default().push(&psi(0, &section)).is_err());
        }
    }

    #[test]
    fn first_idr_does_not_bypass_transport_validation_for_the_rest_of_the_write() {
        let mut gate = StartGate::default();
        tables(&mut gate);
        let keyframe = packet(0x100, true, 0, &[PES, KEYFRAME].concat());
        let mut invalid = packet(0x1fff, false, 0, &[]);
        invalid[0] = 0;
        assert!(gate.push(&[keyframe, invalid].concat()).is_err());
        assert!(gate.push(&keyframe).unwrap().is_some());
    }

    #[test]
    fn changed_pmt_requires_fresh_parameter_sets_and_continuity() {
        for video in [0, 1] {
            let mut gate = StartGate::default();
            tables(&mut gate);
            gate.push(&packet(0x100, true, 4, &[PES, &KEYFRAME[..8]].concat()))
                .unwrap();
            let pmt = program_map(video, 1, 0x1b);
            assert!(gate.push(&pmt).unwrap().is_none());
            let idr = packet(
                0x100 + u16::from(video),
                true,
                5,
                &[PES, &KEYFRAME[8..]].concat(),
            );
            assert!(gate.push(&idr).unwrap().is_none(), "reused old SPS/PPS");
            let keyframe = packet(0x100 + u16::from(video), true, 6, &[PES, KEYFRAME].concat());
            let accepted = gate.push(&keyframe).unwrap().unwrap();
            assert_eq!(&accepted[PACKET..2 * PACKET], &pmt);
            assert_eq!(
                accepted.len(),
                4 * PACKET,
                "retained obsolete program packets"
            );
        }
    }

    #[test]
    fn pmt_without_h264_cannot_reuse_the_previous_video() {
        let mut gate = StartGate::default();
        tables(&mut gate);
        gate.push(&program_map(0, 1, 0x0f)).unwrap();
        assert!(
            gate.push(&packet(0x100, true, 0, &[PES, KEYFRAME].concat()))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn repeated_pmt_preserves_an_in_progress_keyframe() {
        let mut gate = StartGate::default();
        tables(&mut gate);
        gate.push(&packet(0x100, true, 15, &[PES, &KEYFRAME[..8]].concat()))
            .unwrap();
        gate.push(&program_map(0, 0, 0x1b)).unwrap();
        assert!(
            gate.push(&packet(0x100, false, 0, &KEYFRAME[8..]))
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn changed_split_pmt_keeps_the_last_valid_pat_and_all_pmt_packets() {
        let mut gate = StartGate::default();
        tables(&mut gate);
        let pat = gate.pending[..PACKET].to_vec();
        let mut invalid_pat = pat.clone();
        *invalid_pat.last_mut().unwrap() ^= 1; // Corrupt the section CRC.
        gate.push(&invalid_pat).unwrap();
        let pmt = program_map(1, 1, 0x1b);
        let body = payload(&pmt).unwrap();
        let first = packet(0x1000, true, 7, &body[..10]);
        let second = packet(0x1000, false, 8, &body[10..]);
        gate.push(&first).unwrap();
        gate.push(&second).unwrap();
        let keyframe = packet(0x101, true, 3, &[PES, KEYFRAME].concat());
        let accepted = gate.push(&keyframe).unwrap().unwrap();
        assert_eq!(
            accepted,
            [pat.as_slice(), &first, &second, &keyframe].concat()
        );
    }

    #[test]
    fn pmt_rejects_descriptors_extending_past_the_section() {
        for index in [11, 16] {
            let mut gate = StartGate::default();
            tables(&mut gate);
            let pmt = program_map(0, 1, 0x1b);
            let body = payload(&pmt).unwrap();
            let mut section = body[1..body.len() - 4].to_vec();
            section[index] = 32;
            assert!(gate.push(&psi(0x1000, &section)).is_err());
        }
    }

    #[test]
    fn split_optional_pes_header_is_never_scanned_as_video() {
        let mut gate = StartGate::default();
        tables(&mut gate);
        let mut body = PES.to_vec();
        body[8] = 255;
        body.extend_from_slice(KEYFRAME); // These bytes belong to the PES header.
        body.resize(264, 0xff);
        for (index, part) in body.chunks(180).enumerate() {
            assert!(
                gate.push(&packet(0x100, index == 0, index as u8, part))
                    .unwrap()
                    .is_none()
            );
        }
        assert!(
            gate.push(&packet(0x100, false, 2, KEYFRAME))
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn continuation_without_a_pes_start_cannot_open_the_gate() {
        let mut gate = StartGate::default();
        tables(&mut gate);
        assert!(
            gate.push(&packet(0x100, false, 7, KEYFRAME))
                .unwrap()
                .is_none()
        );
        let body = [PES, KEYFRAME].concat();
        let accepted = gate.push(&packet(0x100, true, 8, &body)).unwrap().unwrap();
        assert_eq!(accepted.len(), 3 * PACKET);
    }

    #[test]
    fn new_pat_discards_an_unfinished_pes_header() {
        let mut gate = StartGate::default();
        tables(&mut gate);
        assert!(
            gate.push(&packet(0x100, true, 0, &PES[..2]))
                .unwrap()
                .is_none()
        );
        tables(&mut gate);
        assert!(
            gate.push(&packet(0x100, false, 1, KEYFRAME))
                .unwrap()
                .is_none()
        );
        assert!(gate.push(&packet(0x100, true, 2, PES)).unwrap().is_none());
        assert!(
            gate.push(&packet(0x100, false, 3, KEYFRAME))
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn lost_video_packet_cannot_publish_a_broken_start_point() {
        let mut gate = StartGate::default();
        tables(&mut gate);
        let body = [PES, &KEYFRAME[..8]].concat();
        assert!(
            gate.push(&packet(0x100, true, 14, &body))
                .unwrap()
                .is_none()
        );
        assert!(gate.push(&packet(0x100, false, 0, &KEYFRAME[8..])).is_err());
    }

    #[test]
    fn split_pes_and_start_codes_survive_continuity_wrap_and_adaptation_only() {
        let body = [PES, KEYFRAME].concat();
        for cut in 1..body.len() {
            let mut gate = StartGate::default();
            tables(&mut gate);
            assert!(
                gate.push(&packet(0x100, true, 15, &body[..cut]))
                    .unwrap()
                    .is_none()
            );
            let mut adaptation = [0xff; PACKET];
            adaptation[..6].copy_from_slice(&[0x47, 1, 0, 0x2f, 183, 0]);
            assert!(gate.push(&adaptation).unwrap().is_none());
            assert!(
                gate.push(&packet(0x100, false, 0, &body[cut..]))
                    .unwrap()
                    .is_some()
            );
        }
    }

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
