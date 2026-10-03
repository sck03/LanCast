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
for path in (root / "apple").rglob("*.swift"):
    if any(part in {".build", "Dependencies", ".swiftpm"} for part in path.parts):
        continue
    text = path.read_text(encoding="utf-8")
    if path.name != "CoreSession.swift":
        assert "lancast_" not in text, f"Apple C ABI leaked outside CoreSession: {path}"
    if "Contracts" in path.parts and "Tests" not in path.parts:
        for forbidden in ("import SwiftUI", "import LiveKitWebRTC", "import ReplayKit", "import ScreenCaptureKit", "FileManager", "URLSession"):
            assert forbidden not in text, f"Apple contract dependency leak: {path}: {forbidden}"
    if path.parent.name == "Media":
        for forbidden in ("import SwiftUI", "CoreSession", "SenderModel", "ReceiverModel"):
            assert forbidden not in text, f"Apple media depends on application/control: {path}"
print("PASS: Rust and Apple dependency directions, system Legacy player, no required GStreamer")
