#!/usr/bin/env python3
"""Verify package hashes, PE imports and release TS symbol stripping."""
import argparse
import json
from pathlib import Path
from windows_artifacts import verify, write_hashes


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", nargs="?", type=Path,
                        default=Path(__file__).resolve().parents[1] / "dist/LanCast-Windows-x64",
                        help="Extracted Windows application directory")
    parser.add_argument("--write-hashes", action="store_true", help="Write portable hashes for a freshly staged package before validation")
    args = parser.parse_args()
    root = args.root
    if args.write_hashes:
        write_hashes(root)
    report = verify(root)
    (root / "binary-dependencies.json").write_text(json.dumps(report, indent=2), encoding="utf-8")
    print("PASS: package SHA-256, x64 PE imports, runtime dependencies and Release TS symbol policy")


if __name__ == "__main__":
    main()
