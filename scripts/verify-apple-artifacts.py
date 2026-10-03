#!/usr/bin/env python3
"""Verify downloaded Apple Actions app archives without requiring a Mac.

Usage: python scripts/verify-apple-artifacts.py .cache/artifacts/RUN_ID
This inspects Mach-O headers, archive hashes, Info.plist and extension metadata;
it does not execute apps or verify device signing and runtime behavior.
"""
import argparse
import hashlib
import json
import pathlib
import plistlib
import struct
import zipfile

CPU = {0x01000007: "x86_64", 0x0100000C: "arm64"}


def architectures(data):
    magic = data[:4]
    if magic in (b"\xca\xfe\xba\xbe", b"\xca\xfe\xba\xbf"):
        count = struct.unpack_from(">I", data, 4)[0]
        assert 0 < count <= 8, "Invalid Mach-O architecture count"
        stride = 20 if magic[-1] == 0xBE else 32
        return sorted(CPU[struct.unpack_from(">I", data, 8 + n * stride)[0]] for n in range(count))
    assert magic == b"\xcf\xfa\xed\xfe", "Expected a 64-bit Mach-O executable"
    return [CPU[struct.unpack_from("<I", data, 4)[0]]]


def verify(root):
    reports, packages = [], []
    for platform, scheme, minimum in [("macos", "LanCastMac", "13.0"), ("ios", "LanCastIOS", "16.0"), ("tvos", "LanCastTV", "17.0")]:
        directory = root / f"LanCast-Apple-{platform}-development/dist/apple/{platform}"
        report = json.loads((directory / "build-report.json").read_text(encoding="utf-8"))
        assert report["platform"] == platform
        reports.append(report)
        for entry in report["artifacts"]:
            path = directory / entry["file"]
            assert path.resolve().is_relative_to(directory.resolve())
            assert path.stat().st_size == entry["bytes"]
            assert hashlib.sha256(path.read_bytes()).hexdigest() == entry["sha256"]
            with zipfile.ZipFile(path) as archive:
                names = archive.namelist()
                assert not any(".xctest/" in name for name in names), "Test bundle in app archive"
                base = f"{scheme}.app/" + ("Contents/" if platform == "macos" else "")
                info = plistlib.loads(archive.read(base + "Info.plist"))
                assert info.get("LSMinimumSystemVersion", info.get("MinimumOSVersion")) == minimum
                assert info["NSLocalNetworkUsageDescription"]
                assert "_lancast._tcp" in info["NSBonjourServices"]
                executable = base + ("MacOS/" if platform == "macos" else "") + scheme
                arch = architectures(archive.read(executable))
                product = entry["file"].removeprefix(scheme + "-").removesuffix(".zip")
                expected = next(b["architectures"] for b in report["binaries"] if b["product"] == product)
                assert arch == sorted(expected)
                if platform == "macos":
                    assert arch == ["arm64", "x86_64"]
                framework = base + "Frameworks/LiveKitWebRTC.framework/" + ("Versions/A/" if platform == "macos" else "") + "LiveKitWebRTC"
                assert set(arch) <= set(architectures(archive.read(framework)))
                if platform == "ios":
                    extension = plistlib.loads(archive.read(base + "PlugIns/LanCastBroadcast.appex/Info.plist"))
                    assert extension["LanCastAppGroup"] == info["LanCastAppGroup"]
                    assert extension["NSExtension"]["NSExtensionPointIdentifier"] == "com.apple.broadcast-services-upload"
                    assert extension["NSExtension"]["RPBroadcastProcessMode"] == "RPBroadcastProcessModeSampleBuffer"
                packages.append({"file": entry["file"], "architectures": arch, "minimumOS": minimum,
                    "bytes": entry["bytes"], "sha256": entry["sha256"], "metadataAndFramework": "passed", "testBundleAbsent": True})
    commits = {report["commit"] for report in reports}
    assert len(commits) == 1, "Mixed source commits"
    return {"commit": commits.pop(), "scope": "Offline archive verification; no runtime or signing acceptance", "packages": packages, "buildReports": reports}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("artifacts", type=pathlib.Path)
    args = parser.parse_args()
    print(json.dumps(verify(args.artifacts), indent=2) + "\n")
