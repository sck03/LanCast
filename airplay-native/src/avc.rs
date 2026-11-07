//! Checked AVC configuration and access-unit conversion, independent of Android.
use std::io;
const MAX_FRAME_SIZE: usize = 2 * 1024 * 1024;
fn bad() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid H.264 access unit")
}

#[derive(Clone, Debug, PartialEq)]
pub struct AvcConfig {
    pub width: u32,
    pub height: u32,
    pub sps: Vec<u8>,
    pub pps: Vec<u8>,
    pub length_size: usize,
}
impl AvcConfig {
    pub fn parse(data: &[u8]) -> io::Result<Self> {
        if data.len() < 7 || data[0] != 1 || data.len() > 65536 {
            return Err(bad());
        }
        let length_size = usize::from(data[4] & 3) + 1;
        // Three-byte lengths are reserved by AVCDecoderConfigurationRecord.
        if length_size == 3 {
            return Err(bad());
        }
        let mut pos = 6;
        let mut sps = None;
        for _ in 0..(data[5] & 31) {
            let n = nal(data, &mut pos)?;
            if n[0] & 0x9f != 7 {
                return Err(bad());
            }
            if sps.is_none() {
                sps = Some(n.to_vec());
            }
        }
        let count = *data.get(pos).ok_or_else(bad)?;
        pos += 1;
        let mut pps = None;
        for _ in 0..count {
            let n = nal(data, &mut pos)?;
            if n[0] & 0x9f != 8 {
                return Err(bad());
            }
            if pps.is_none() {
                pps = Some(n.to_vec());
            }
        }
        let sps = sps.ok_or_else(bad)?;
        let pps = pps.ok_or_else(bad)?;
        let (width, height) = dimensions(&sps)?;
        Ok(Self {
            width,
            height,
            sps,
            pps,
            length_size,
        })
    }
    pub fn annex_b(&self, data: &[u8]) -> io::Result<(Vec<u8>, bool)> {
        if data.is_empty() || data.len() > MAX_FRAME_SIZE || !matches!(self.length_size, 1 | 2 | 4)
        {
            return Err(bad());
        }
        let (size, key) = self.scan(data)?;
        let mut result = Vec::with_capacity(size);
        let mut pos = 0;
        while pos < data.len() {
            let n = data[pos..pos + self.length_size]
                .iter()
                .fold(0usize, |n, b| (n << 8) | usize::from(*b));
            pos += self.length_size;
            result.extend_from_slice(&[0, 0, 0, 1]);
            result.extend_from_slice(&data[pos..pos + n]);
            pos += n;
        }
        Ok((result, key))
    }
    // Validate before allocating/copying, including expansion of short NAL lengths.
    fn scan(&self, data: &[u8]) -> io::Result<(usize, bool)> {
        let (mut pos, mut size, mut count, mut key) = (0, 0usize, 0, false);
        while pos < data.len() {
            let length = data.get(pos..pos + self.length_size).ok_or_else(bad)?;
            let n = length
                .iter()
                .fold(0usize, |n, b| (n << 8) | usize::from(*b));
            pos += self.length_size;
            if n == 0 || n > data.len() - pos || count >= 1024 {
                return Err(bad());
            }
            let header = data[pos];
            if header & 0x80 != 0 || !matches!(header & 31, 1..=23) {
                return Err(bad());
            }
            size += 4 + n;
            if size > MAX_FRAME_SIZE {
                return Err(bad());
            }
            key |= header & 31 == 5;
            count += 1;
            pos += n;
        }
        Ok((size, key))
    }
}
fn nal<'a>(data: &'a [u8], pos: &mut usize) -> io::Result<&'a [u8]> {
    let header = data.get(*pos..*pos + 2).ok_or_else(bad)?;
    let n = usize::from(u16::from_be_bytes(header.try_into().unwrap()));
    *pos += 2;
    if n == 0 {
        return Err(bad());
    }
    let value = data.get(*pos..*pos + n).ok_or_else(bad)?;
    *pos += n;
    Ok(value)
}
struct Bits {
    bytes: Vec<u8>,
    bit: usize,
}
impl Bits {
    fn read(&mut self, n: usize) -> io::Result<u32> {
        if n > 32 || self.bit + n > self.bytes.len() * 8 {
            return Err(bad());
        }
        let mut value = 0;
        for _ in 0..n {
            value = (value << 1) | u32::from((self.bytes[self.bit / 8] >> (7 - self.bit % 8)) & 1);
            self.bit += 1;
        }
        Ok(value)
    }
    fn ue(&mut self) -> io::Result<u32> {
        let mut zeros = 0;
        while self.read(1)? == 0 {
            zeros += 1;
            if zeros > 24 {
                return Err(bad());
            }
        }
        Ok(((1u32 << zeros) - 1) + self.read(zeros)?)
    }
    fn se(&mut self) -> io::Result<i32> {
        let v = self.ue()?;
        Ok(if v & 1 != 0 {
            (v as i32 + 1) / 2
        } else {
            -(v as i32) / 2
        })
    }
}
pub fn dimensions(sps: &[u8]) -> io::Result<(u32, u32)> {
    if sps.len() < 4 {
        return Err(bad());
    }
    let mut bytes = Vec::new();
    let mut zeros = 0;
    for &b in &sps[1..] {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        bytes.push(b);
        zeros = if b == 0 { zeros + 1 } else { 0 };
    }
    let mut b = Bits { bytes, bit: 0 };
    let profile = b.read(8)?;
    b.read(16)?;
    b.ue()?;
    let mut chroma = 1;
    if [100, 110, 122, 244, 44, 83, 86, 118, 128, 138, 139, 134, 135].contains(&profile) {
        chroma = b.ue()?;
        if chroma > 3 {
            return Err(bad());
        }
        if chroma == 3 {
            b.read(1)?;
        }
        b.ue()?;
        b.ue()?;
        b.read(1)?;
        if b.read(1)? != 0 {
            for i in 0..if chroma == 3 { 12 } else { 8 } {
                if b.read(1)? != 0 {
                    let mut last = 8;
                    let mut next = 8;
                    for _ in 0..if i < 6 { 16 } else { 64 } {
                        if next != 0 {
                            next = (last + b.se()?).rem_euclid(256);
                        }
                        if next != 0 {
                            last = next;
                        }
                    }
                }
            }
        }
    }
    b.ue()?;
    match b.ue()? {
        0 => {
            b.ue()?;
        }
        1 => {
            b.read(1)?;
            b.se()?;
            b.se()?;
            let n = b.ue()?;
            if n > 255 {
                return Err(bad());
            }
            for _ in 0..n {
                b.se()?;
            }
        }
        2 => {}
        _ => return Err(bad()),
    }
    b.ue()?;
    b.read(1)?;
    let w = b.ue()? + 1;
    let h = b.ue()? + 1;
    let frame = b.read(1)?;
    if w > 256 || h > 256 {
        return Err(bad());
    }
    if frame == 0 {
        b.read(1)?;
    }
    b.read(1)?;
    let (left, right, top, bottom) = if b.read(1)? != 0 {
        (b.ue()?, b.ue()?, b.ue()?, b.ue()?)
    } else {
        (0, 0, 0, 0)
    };
    let crop_x = if chroma == 1 || chroma == 2 { 2 } else { 1 };
    let crop_y = if chroma == 1 { 2 } else { 1 } * (2 - frame);
    let width = (w * 16)
        .checked_sub((left + right) * crop_x)
        .ok_or_else(bad)?;
    let height = (h * 16 * (2 - frame))
        .checked_sub((top + bottom) * crop_y)
        .ok_or_else(bad)?;
    if !(16..=4096).contains(&width) || !(16..=4096).contains(&height) {
        return Err(bad());
    }
    Ok((width, height))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_lengths_are_bounded() {
        let c = AvcConfig {
            width: 1,
            height: 1,
            sps: vec![],
            pps: vec![],
            length_size: 4,
        };
        assert!(c.annex_b(&[0xff; 4]).is_err());
        assert!(AvcConfig::parse(&[1; 6]).is_err());
        let (data, key) = c.annex_b(&[0, 0, 0, 2, 0x65, 1]).unwrap();
        assert!(key);
        assert_eq!(data, [0, 0, 0, 1, 0x65, 1]);
    }

    #[test]
    fn rejects_reserved_lengths_and_invalid_nal_headers() {
        let fixture = include_bytes!("../tests/fixtures/rx580-config.avcc");
        let mut config = AvcConfig::parse(fixture).unwrap();
        let mut reserved = fixture.to_vec();
        reserved[4] = (reserved[4] & !3) | 2;
        assert!(AvcConfig::parse(&reserved).is_err());
        for size in [0, 3, 5, usize::MAX] {
            config.length_size = size;
            assert!(config.annex_b(&[1, 0x65]).is_err());
        }
        config.length_size = 1;
        for header in [0, 0x80, 0xe5, 24] {
            assert!(config.annex_b(&[1, header]).is_err());
        }
        assert_eq!(
            config.annex_b(&[1, 0x65, 1, 0x41]).unwrap(),
            (vec![0, 0, 0, 1, 0x65, 0, 0, 0, 1, 0x41], true)
        );
    }

    #[test]
    fn converted_size_cannot_exceed_decoder_capacity() {
        let mut config =
            AvcConfig::parse(include_bytes!("../tests/fixtures/rx580-config.avcc")).unwrap();
        config.length_size = 2;
        let mut frame = Vec::new();
        // 33 legal NAL units fit the input limit but exceed it after Annex B expansion.
        for _ in 0..32 {
            frame.extend_from_slice(&65533u16.to_be_bytes());
            frame.push(0x41);
            frame.resize(frame.len() + 65532, 0);
        }
        frame.extend_from_slice(&[0, 30, 0x41]);
        frame.resize(frame.len() + 29, 0);
        assert_eq!(frame.len(), MAX_FRAME_SIZE);
        assert!(config.annex_b(&frame).is_err());
    }
}
