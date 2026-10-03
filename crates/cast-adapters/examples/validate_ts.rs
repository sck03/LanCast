//! CI-only validator for a real FFmpeg-generated synthetic stream.
use cast_adapters::ts::StartGate;
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
    // Include a mid-GOP join and start-code boundaries at a single TS packet per write.
    for (index, offset) in [0, 188 * 50, 188 * 173].into_iter().enumerate() {
        let mut gate = StartGate::default();
        let mut file = std::fs::File::create(format!("{}/join-{index}.ts", args[2]))?;
        let mut count = 0;
        for packet in input[offset..].as_chunks::<188>().0 {
            if let Some(bytes) = gate.push(packet)? {
                count += bytes.len();
                file.write_all(&bytes)?;
            }
        }
        anyhow::ensure!(count > 188 * 20, "no valid random access point");
        println!("join={index} offset={offset} acceptedBytes={count}");
    }
    Ok(())
}
