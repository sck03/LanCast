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
from apple_products import PRODUCTS

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


def find_builds(roots):
    found = {}
    for root in roots:
        assert root.is_dir(), f"Missing artifact directory: {root}"
        root_found = False
        for path in root.rglob("build-report.json"):
            data = json.loads(path.read_text(encoding="utf-8"))
            platform = data.get("platform")
            if platform in ("macos", "ios", "tvos"):
                root_found = True
                assert platform not in found, "Multiple builds for one platform; select exact run directories"
                found[platform] = path.parent
        assert root_found, f"No Apple build report in requested directory: {root}"
    assert found, "No Apple build reports found"
    return found


def verify(*roots):
    reports, packages = [], []
    found = find_builds(roots)
    for platform, scheme, minimum in [("macos", "LanCastMac", "13.0"), ("ios", "LanCastIOS", "16.0"), ("tvos", "LanCastTV", "17.0")]:
        if platform not in found:
            continue
        directory = found[platform]
        report = json.loads((directory / "build-report.json").read_text(encoding="utf-8"))
        assert report["platform"] == platform
        definition = PRODUCTS[platform]
        if "role" in report:
            assert report["role"] == definition.role, "Product role mismatch"
            assert report["rustFeatures"] == definition.rust_features.split(","), "Product feature mismatch"
        reports.append(report)
        assert len(report["artifacts"]) == (1 if platform == "macos" else 2)
        if "role" in report:
            expected_products = {name for name, _ in definition.directories(report["buildConfig"]["configuration"])}
            assert {entry["product"] for entry in report["artifacts"]} == expected_products, "Missing or duplicate device/simulator archive"
            assert {entry["product"] for entry in report["binaries"]} == expected_products, "Binary target mismatch"
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
                assert info["CFBundleIdentifier"] == definition.bundle_id, "Application identity mismatch"
                if "buildConfig" in report:
                    config = report["buildConfig"]
                    assert info["CFBundleShortVersionString"] == config["version"], "Application version mismatch"
                    assert str(info["CFBundleVersion"]) == str(config["build_number"]), "Application build number mismatch"
                    assert report["commit"] == config["source_commit"], "Source provenance mismatch"
                assert info.get("LSMinimumSystemVersion", info.get("MinimumOSVersion")) == minimum
                assert info["NSLocalNetworkUsageDescription"]
                assert "_lancast._tcp" in info["NSBonjourServices"]
                executable = base + ("MacOS/" if platform == "macos" else "") + scheme
                arch = architectures(archive.read(executable))
                product = entry.get("product") or entry["file"].removeprefix(scheme + "-").removesuffix(".zip")
                expected = next(b["architectures"] for b in report["binaries"] if b["product"] == product)
                assert arch == sorted(expected)
                if platform == "macos":
                    assert arch == ["arm64", "x86_64"]
                else:
                    assert arch == ["arm64"]
                    simulator = product.endswith("simulator")
                    supported = ("iPhoneSimulator" if simulator else "iPhoneOS") if platform == "ios" else ("AppleTVSimulator" if simulator else "AppleTVOS")
                    assert info["CFBundleSupportedPlatforms"] == [supported], "Device/simulator target mismatch"
                    if platform == "tvos":
                        assert info["UIDeviceFamily"] == [3], "tvOS must target Apple TV"
                        assert not any(".appex/" in name for name in names), "Broadcast extension in tvOS receiver"
                framework = base + "Frameworks/LiveKitWebRTC.framework/" + ("Versions/A/" if platform == "macos" else "") + "LiveKitWebRTC"
                assert set(arch) <= set(architectures(archive.read(framework)))
                if platform == "ios":
                    extension = plistlib.loads(archive.read(base + "PlugIns/LanCastBroadcast.appex/Info.plist"))
                    assert extension["LanCastAppGroup"] == info["LanCastAppGroup"]
                    assert extension["CFBundleShortVersionString"] == info["CFBundleShortVersionString"]
                    assert extension["CFBundleVersion"] == info["CFBundleVersion"]
                    assert extension["NSExtension"]["NSExtensionPointIdentifier"] == "com.apple.broadcast-services-upload"
                    assert extension["NSExtension"]["RPBroadcastProcessMode"] == "RPBroadcastProcessModeSampleBuffer"
                packages.append({"file": entry["file"], "architectures": arch, "minimumOS": minimum,
                    "version": info.get("CFBundleShortVersionString"), "buildNumber": info.get("CFBundleVersion"),
                    "bytes": entry["bytes"], "sha256": entry["sha256"], "metadataAndFramework": "passed", "testBundleAbsent": True})
    commits = {report["commit"] for report in reports}
    assert len(commits) == 1, "Mixed source commits"
    return {"commit": commits.pop(), "scope": "Offline archive verification; no runtime or signing acceptance", "packages": packages, "buildReports": reports}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("artifacts", type=pathlib.Path, nargs="+")
    args = parser.parse_args()
    print(json.dumps(verify(*args.artifacts), indent=2) + "\n")
