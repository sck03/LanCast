#!/usr/bin/env python3
"""Fail on forbidden dependency directions or a regression to the old media runtime."""
import pathlib
import tomllib
root = pathlib.Path(__file__).resolve().parents[1]
rules = {"crates/cast-domain": {"serde"}, "crates/cast-core": {"cast-domain"}, "core": {"cast-adapters", "serde_json"}}
for directory, allowed in rules.items():
    manifest = tomllib.loads((root / directory / "Cargo.toml").read_text(encoding="utf-8"))
    actual = set(manifest.get("dependencies", {}))
    assert actual <= allowed, f"{directory}: forbidden dependencies {actual - allowed}"
for path in (root / "crates/cast-domain/src").glob("*.rs"):
    text = path.read_text(encoding="utf-8")
    for forbidden in ("std::fs", "std::net", "tokio::", "android", "windows::"):
        assert forbidden not in text, f"Domain I/O leak: {path}: {forbidden}"
legacy = (root / "android/player-legacy/build.gradle.kts").read_text(encoding="utf-8")
assert "androidx.media3" not in legacy
assert "gstreamer" not in (root / "windows/CMakeLists.txt").read_text(encoding="utf-8").lower()
assert "gst_" not in (root / "windows/src/main.cpp").read_text(encoding="utf-8")
print("PASS: domain/core/FFI dependency directions, system Legacy player, no required GStreamer")
