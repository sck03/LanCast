#!/usr/bin/env python3
"""Decode media_probe's synthetic TS; check content and timestamps without NumPy or a TV."""
import argparse
import array
import hashlib
import json
import math
from pathlib import Path
import re
import subprocess


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("input", type=Path)
    parser.add_argument("--ffmpeg", default="ffmpeg")
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--commit", required=True)
    args = parser.parse_args()

    def decode(options, debug=False):
        command = [args.ffmpeg, "-hide_banner", "-v", "info" if debug else "error"]
        if debug:
            command += ["-debug_ts"]
        command += ["-xerror", "-i", str(args.input), *options]
        result = subprocess.run(command, capture_output=True, check=True)
        log = result.stderr.decode("utf-8", errors="replace")
        if re.search(r"non monotonically|error while|invalid data|corrupt decoded", log, re.I):
            raise RuntimeError(log)
        return result.stdout, log

    # Preserve the source timebase. A null muxer's inferred frame-rate timebase can otherwise
    # round distinct VFR timestamps to the same integer, which is not an input timestamp error.
    _, log = decode(["-fps_mode", "passthrough", "-enc_time_base:v", "demux", "-f", "null", "-"], debug=True)
    rows = [tuple(map(int, row)) for row in re.findall(r"demuxer -> ist_index:0:0 type:video pkt_pts:(-?\d+).*?pkt_dts:(-?\d+)", log)]
    if len(rows) < 100 or not all(pts == dts for pts, dts in rows):
        raise RuntimeError("Missing timestamps, B-frames or too few video packets")
    differences = [b[0] - a[0] for a, b in zip(rows, rows[1:])]
    if min(differences) <= 0:
        raise RuntimeError("Input timestamps are not strictly increasing")
    video = next(line.strip() for line in log.splitlines() if "Video: h264" in line)
    audio = next(line.strip() for line in log.splitlines() if "Audio: aac" in line)
    if any(part not in video for part in ("1280x720", "Baseline", "bt709", "90k tbn")) or any(part not in audio for part in ("(LC)", "48000 Hz", "stereo")):
        raise RuntimeError("Unexpected codec profile, dimensions, color metadata or audio format")
    colors = []
    for index in range(3):
        # Output-side seek decodes from the beginning; input-side fast seeking on an unindexed
        # live TS can land on the following IDR rather than the requested instant.
        pixels, _ = decode(["-ss", str(index + .5), "-frames:v", "1", "-vf", "scale=1:1", "-pix_fmt", "rgb24", "-f", "rawvideo", "-"])
        color = list(pixels)
        if len(color) != 3 or color[index] < 240 or max(color[:index] + color[index + 1:]) > 20:
            raise RuntimeError(f"Unexpected color at {index+.5}s: {color}")
        colors.append(color)
    pcm, _ = decode(["-map", "0:a:0", "-t", "1", "-ar", "48000", "-ac", "1", "-f", "f32le", "-"])
    samples = array.array("f")
    samples.frombytes(pcm)
    import sys
    if sys.byteorder != "little":
        samples.byteswap()

    def power(frequency):
        coefficient = 2 * math.cos(2 * math.pi * frequency / 48000)
        previous = before = 0.0
        for sample in samples:
            value = sample + coefficient * previous - before
            before, previous = previous, value
        return previous**2 + before**2 - coefficient * previous * before

    peak = max(range(430, 451), key=power)
    rms = math.sqrt(sum(sample * sample for sample in samples) / len(samples))
    if abs(peak - 440) > 1 or rms < .02:
        raise RuntimeError("Missing or incorrect synthetic audio tone")
    decoder = subprocess.check_output([args.ffmpeg, "-version"]).decode("utf-8", errors="replace").splitlines()[0]
    report = {"schemaVersion": 1, "commit": args.commit, "decoder": decoder, "fixture": args.input.name,
              "sha256": hashlib.sha256(args.input.read_bytes()).hexdigest(),
              "bytes": args.input.stat().st_size, "video": video, "audio": audio,
              "videoPackets": len(rows), "ptsEqualsDts": True, "strictlyIncreasing": True,
              "minStep90k": min(differences), "maxStep90k": max(differences),
              "spanSeconds": (rows[-1][0] - rows[0][0]) / 90000,
              "rgbAtSeconds": [.5, 1.5, 2.5], "rgb": colors,
              "audioPeakHz": peak, "audioRms": rms,
              "scope": "synthetic GPU/MFT/TS decode only; excludes screen capture, loopback audio, TV display and end-to-end latency"}
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, indent=2), encoding="utf-8")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
