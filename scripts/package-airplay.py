#!/usr/bin/env python3
"""Archive the exact optional GPL sources and lock-resolved Rust dependency sources."""
import hashlib
import json
from pathlib import Path
import subprocess
import tarfile

ROOT = Path(__file__).resolve().parents[1]


def main():
    dest = ROOT / "dist/airplay-evidence"
    dest.mkdir(parents=True, exist_ok=True)
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    subprocess.run(["git", "archive", "--format=tar.gz", "-o", str(dest / "lancast-source.tar.gz"), revision], cwd=ROOT, check=True)
    vendor = ROOT / ".cache/airplay-cargo-sources"
    config = subprocess.check_output(["cargo", "vendor", "--locked", "--versioned-dirs", "--manifest-path", "airplay-native/Cargo.toml", "--sync", "Cargo.toml", str(vendor)], cwd=ROOT, text=True)
    # Use a relative path when unpacking in a different checkout.
    config = config.replace(str(vendor).replace("\\", "\\\\"), "cargo-sources").replace(vendor.as_posix(), "cargo-sources")
    with tarfile.open(dest / "rust-dependency-sources.tar.gz", "w:gz") as archive:
        archive.add(vendor, arcname="cargo-sources")
    (dest / "cargo-config.toml").write_text(config, encoding="utf-8")
    metadata = json.loads(subprocess.check_output(["cargo", "metadata", "--manifest-path", "airplay-native/Cargo.toml", "--locked", "--format-version", "1"], cwd=ROOT))
    inventory = [{key: package.get(key) for key in ("name", "version", "license", "source", "repository")} for package in metadata["packages"]]
    (dest / "dependencies.json").write_text(json.dumps({"source_commit": revision, "packages": inventory}, indent=2) + "\n", encoding="utf-8")
    (dest / "REBUILD.txt").write_text(
        "Optional LanCast AirPlay receiver, GPL-3.0-only as a combined work.\n"
        "Extract lancast-source.tar.gz and rust-dependency-sources.tar.gz into one folder.\n"
        "Copy cargo-config.toml to .cargo/config.toml. Follow .github/workflows/android.yml\n"
        "with JDK 17, Android SDK 36, NDK 27.2.12479018 and Rust 1.98.1.\n"
        "The reviewed rairplay/PlayFair sources and original hashes are in airplay-native/vendor.\n"
        "Modify sources, rebuild JNI and APK, then sign with your own key; no signature gate is imposed.\n"
        "Android SDK/JDK and Maven artifacts are fetched by their normal build tools.\n"
        "The existing fixed WebRTC AAR remains a supplier binary; see THIRD_PARTY.md.\n"
        "This is review/build evidence. Physical iPhone/TV interop and production release remain unverified.\n", encoding="utf-8")
    files = [{"file": p.name, "bytes": p.stat().st_size, "sha256": hashlib.sha256(p.read_bytes()).hexdigest()} for p in sorted(dest.iterdir()) if p.is_file() and p.name != "manifest.json"]
    (dest / "manifest.json").write_text(json.dumps({"schema": 1, "commit": revision, "license": "GPL-3.0-only", "files": files}, indent=2) + "\n", encoding="utf-8")
    print(f"AirPlay source and dependency evidence: {dest}")


if __name__ == "__main__":
    main()
