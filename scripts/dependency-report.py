#!/usr/bin/env python3
"""Generate the resolved Rust license/source inventory. Not a complete binary SBOM."""
import json
from pathlib import Path
import subprocess
root = Path(__file__).resolve().parents[1]
metadata = json.loads(subprocess.check_output(["cargo", "metadata", "--locked", "--format-version=1", "--all-features"], cwd=root))
out = root / "dist/reports"
out.mkdir(parents=True, exist_ok=True)
packages = [{k: p.get(k) for k in ("name", "version", "license", "license_file", "source", "repository")} for p in metadata["packages"]]
(out / "rust-dependencies.json").write_text(json.dumps(packages, ensure_ascii=False, indent=2), encoding="utf-8")
print(f"Recorded {len(packages)} resolved package entries")
