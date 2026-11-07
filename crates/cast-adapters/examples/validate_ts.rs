//! CI-only validator for a real FFmpeg-generated synthetic stream.
use cast_adapters::ts::{MAX_WRITE, PACKET, StartGate};
use std::io::Write;
fn main() -> anyhow::Result<()> {
    let args: Vec<_> = std::env::args().collect();
    anyhow::ensure!(
        args.len() == 3,
        "usage: validate_ts input.ts output-directory"
    );
    let input = std::fs::read(&args[1])?;
    anyhow::ensure!(
        input.len() > 188 * 348 && input.len().is_multiple_of(188),
        "invalid fixture"
    );
    std::fs::create_dir_all(&args[2])?;
    // A mux may flush one packet or a whole block: joining must produce identical TS.
    for (index, offset) in [0, 188 * 50, 188 * 173].into_iter().enumerate() {
        let mut reference = None;
        for packets in [1, 7, MAX_WRITE / PACKET] {
            let mut gate = StartGate::default();
            let mut output = Vec::new();
            for block in input[offset..].chunks(packets * PACKET) {
                if let Some(bytes) = gate.push(block)? {
                    output.extend_from_slice(&bytes);
                }
            }
            anyhow::ensure!(output.len() > PACKET * 20, "no valid random access point");
            if let Some(expected) = &reference {
                anyhow::ensure!(
                    &output == expected,
                    "write boundaries changed startup output"
                );
            }
            std::fs::write(format!("{}/join-{index}-{packets}.ts", args[2]), &output)?;
            println!(
                "join={index} offset={offset} packetsPerWrite={packets} acceptedBytes={}",
                output.len()
            );
            reference.get_or_insert(output);
        }
    }
    // Repacketize real PAT/PMT/PES starts at deliberately hostile byte boundaries.
    // The MPEG sections and elementary stream remain byte-identical, including their CRC.
    for cut in [1, 2, 7, 12] {
        let split = split_headers(&input, cut);
        let mut gate = StartGate::default();
        let mut file = std::fs::File::create(format!("{}/split-{cut}.ts", args[2]))?;
        let mut accepted = 0;
        for packet in split.as_chunks::<188>().0 {
            if let Some(bytes) = gate.push(packet)? {
                accepted += bytes.len();
                file.write_all(&bytes)?;
            }
        }
        anyhow::ensure!(accepted > 188 * 20, "split headers failed to start");
        println!("splitHeaderAt={cut} acceptedBytes={accepted}");
    }
    Ok(())
}
fn split_headers(input: &[u8], cut: usize) -> Vec<u8> {
    let mut counters = std::collections::HashMap::<u16, u8>::new();
    let mut output = Vec::new();
    for original in input.as_chunks::<188>().0 {
        let pid = (u16::from(original[1] & 31) << 8) | u16::from(original[2]);
        let start = if original[3] & 0x20 != 0 {
            5 + original[4] as usize
        } else {
            4
        };
        let body = if original[3] & 0x10 != 0 {
            &original[start..]
        } else {
            &[]
        };
        let cc = counters.entry(pid).or_insert(original[3] & 15);
        if original[1] & 0x40 == 0 || body.len() <= cut {
            let mut p = original.to_vec();
            p[3] = (p[3] & 0xf0) | *cc;
            output.extend(p);
            if !body.is_empty() {
                *cc = (*cc + 1) & 15;
            }
            continue;
        }
        for (index, part) in [&body[..cut], &body[cut..]].into_iter().enumerate() {
            let mut p = [0xff; 188];
            p[..4].copy_from_slice(&original[..4]);
            if index > 0 {
                p[1] &= !0x40;
            }
            p[3] = (p[3] & 0xc0) | 0x30 | *cc;
            *cc = (*cc + 1) & 15;
            let length = 183 - part.len();
            p[4] = length as u8;
            if length > 0 {
                p[5] = 0;
                if index == 0 && original[3] & 0x20 != 0 && original[4] > 0 {
                    let n = (original[4] as usize).min(length);
                    p[5..5 + n].copy_from_slice(&original[5..5 + n]);
                }
            }
            p[5 + length..].copy_from_slice(part);
            output.extend_from_slice(&p);
        }
    }
    output
}
