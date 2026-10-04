#!/usr/bin/env python3
"""Collect locked Cargo metadata and write portable source dependency reports."""
import argparse
from datetime import datetime, timezone
import json
from pathlib import Path
import subprocess
import tomllib

from dependency_inventory import build_reports, digest, encode_json


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--offline", action="store_true", help="Require dependencies already in Cargo's cache")
    parser.add_argument("--output", type=Path, default=Path("dist/reports"))
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]

    def git(*argv):
        return subprocess.check_output(["git", *argv], cwd=root, text=True).strip()

    command = ["cargo", "metadata", "--locked", "--format-version=1", "--all-features"]
    if args.offline:
        command.append("--offline")
    metadata = json.loads(subprocess.check_output(command, cwd=root))
    lock_bytes = (root / "Cargo.lock").read_bytes()
    reports = build_reports(metadata, tomllib.loads(lock_bytes.decode("utf-8")), root=root,
                            commit=git("rev-parse", "HEAD"), lock_sha256=digest(lock_bytes),
                            created=git("show", "-s", "--format=%cI", "HEAD"),
                            dirty=bool(git("status", "--porcelain", "--untracked-files=normal")))
    # SPDX timestamps must use UTC, independent of the developer's time zone.
    info = reports["rust-source.spdx.json"]["creationInfo"]
    info["created"] = datetime.fromisoformat(info["created"]).astimezone(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    output = root / args.output
    output.mkdir(parents=True, exist_ok=True)
    for name, report in reports.items():
        (output / name).write_text(encode_json(report), encoding="utf-8", newline="\n")
    print(f"Recorded {len(metadata['packages'])} source packages in {len(reports)} reports; not a binary SBOM")


if __name__ == "__main__":
    main()
