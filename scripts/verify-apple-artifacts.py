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
from artifact_checks import require

CPU = {0x01000007: "x86_64", 0x0100000C: "arm64"}


def architectures(data):
    magic = data[:4]
    if magic in (b"\xca\xfe\xba\xbe", b"\xca\xfe\xba\xbf"):
        count = struct.unpack_from(">I", data, 4)[0]
        require(0 < count <= 8, "Invalid Mach-O architecture count")
        stride = 20 if magic[-1] == 0xBE else 32
        return sorted(CPU[struct.unpack_from(">I", data, 8 + n * stride)[0]] for n in range(count))
    require(magic == b"\xcf\xfa\xed\xfe", "Expected a 64-bit Mach-O executable")
    return [CPU[struct.unpack_from("<I", data, 4)[0]]]


def find_builds(roots):
    found = {}
    for root in roots:
        require(root.is_dir(), f"Missing artifact directory: {root}")
        root_found = False
        for path in root.rglob("build-report.json"):
            data = json.loads(path.read_text(encoding="utf-8"))
            platform = data.get("platform")
            if platform in ("macos", "ios", "tvos"):
                root_found = True
                require(platform not in found, "Multiple builds for one platform; select exact run directories")
                found[platform] = path.parent
        require(root_found, f"No Apple build report in requested directory: {root}")
    require(found, "No Apple build reports found")
    return found


def verify(*roots):
    reports, packages = [], []
    found = find_builds(roots)
    for platform, scheme, minimum in [("macos", "LanCastMac", "13.0"), ("ios", "LanCastIOS", "16.0"), ("tvos", "LanCastTV", "17.0")]:
        if platform not in found:
            continue
        directory = found[platform]
        report = json.loads((directory / "build-report.json").read_text(encoding="utf-8"))
        require(report["platform"] == platform, 'Artifact validation failed')
        definition = PRODUCTS[platform]
        if "role" in report:
            require(report["role"] == definition.role, "Product role mismatch")
            require(report["rustFeatures"] == definition.rust_features.split(","), "Product feature mismatch")
        reports.append(report)
        require(len(report["artifacts"]) == (1 if platform == "macos" else 2), 'Artifact validation failed')
        if "role" in report:
            expected_products = {name for name, _ in definition.directories(report["buildConfig"]["configuration"])}
            require({entry["product"] for entry in report["artifacts"]} == expected_products, "Missing or duplicate device/simulator archive")
            require({entry["product"] for entry in report["binaries"]} == expected_products, "Binary target mismatch")
        for entry in report["artifacts"]:
            path = directory / entry["file"]
            require(path.resolve().is_relative_to(directory.resolve()), 'Artifact validation failed')
            require(path.stat().st_size == entry["bytes"], 'Artifact validation failed')
            require(hashlib.sha256(path.read_bytes()).hexdigest() == entry["sha256"], 'Artifact validation failed')
            with zipfile.ZipFile(path) as archive:
                names = archive.namelist()
                require(not any(".xctest/" in name for name in names), "Test bundle in app archive")
                base = f"{scheme}.app/" + ("Contents/" if platform == "macos" else "")
                info = plistlib.loads(archive.read(base + "Info.plist"))
                require(info["CFBundleIdentifier"] == definition.bundle_id, "Application identity mismatch")
                if "buildConfig" in report:
                    config = report["buildConfig"]
                    require(info["CFBundleShortVersionString"] == config["version"], "Application version mismatch")
                    require(str(info["CFBundleVersion"]) == str(config["build_number"]), "Application build number mismatch")
                    require(report["commit"] == config["source_commit"], "Source provenance mismatch")
                require(info.get("LSMinimumSystemVersion", info.get("MinimumOSVersion")) == minimum, 'Artifact validation failed')
                require(info["NSLocalNetworkUsageDescription"], 'Artifact validation failed')
                require("_lancast._tcp" in info["NSBonjourServices"], 'Artifact validation failed')
                executable = base + ("MacOS/" if platform == "macos" else "") + scheme
                arch = architectures(archive.read(executable))
                product = entry.get("product") or entry["file"].removeprefix(scheme + "-").removesuffix(".zip")
                expected = next(b["architectures"] for b in report["binaries"] if b["product"] == product)
                require(arch == sorted(expected), 'Artifact validation failed')
                if platform == "macos":
                    require(arch == ["arm64", "x86_64"], 'Artifact validation failed')
                else:
                    require(arch == ["arm64"], 'Artifact validation failed')
                    simulator = product.endswith("simulator")
                    supported = ("iPhoneSimulator" if simulator else "iPhoneOS") if platform == "ios" else ("AppleTVSimulator" if simulator else "AppleTVOS")
                    require(info["CFBundleSupportedPlatforms"] == [supported], "Device/simulator target mismatch")
                    if platform == "tvos":
                        require(info["UIDeviceFamily"] == [3], "tvOS must target Apple TV")
                        require(not any(".appex/" in name for name in names), "Broadcast extension in tvOS receiver")
                framework = base + "Frameworks/LiveKitWebRTC.framework/" + ("Versions/A/" if platform == "macos" else "") + "LiveKitWebRTC"
                require(set(arch) <= set(architectures(archive.read(framework))), 'Artifact validation failed')
                if platform == "ios":
                    extension = plistlib.loads(archive.read(base + "PlugIns/LanCastBroadcast.appex/Info.plist"))
                    require(extension["LanCastAppGroup"] == info["LanCastAppGroup"], 'Artifact validation failed')
                    require(extension["CFBundleShortVersionString"] == info["CFBundleShortVersionString"], 'Artifact validation failed')
                    require(extension["CFBundleVersion"] == info["CFBundleVersion"], 'Artifact validation failed')
                    require(extension["NSExtension"]["NSExtensionPointIdentifier"] == "com.apple.broadcast-services-upload", 'Artifact validation failed')
                    require(extension["NSExtension"]["RPBroadcastProcessMode"] == "RPBroadcastProcessModeSampleBuffer", 'Artifact validation failed')
                packages.append({"file": entry["file"], "architectures": arch, "minimumOS": minimum,
                    "version": info.get("CFBundleShortVersionString"), "buildNumber": info.get("CFBundleVersion"),
                    "bytes": entry["bytes"], "sha256": entry["sha256"], "metadataAndFramework": "passed", "testBundleAbsent": True})
    commits = {report["commit"] for report in reports}
    require(len(commits) == 1, "Mixed source commits")
    return {"commit": commits.pop(), "scope": "Offline archive verification; no runtime or signing acceptance", "packages": packages, "buildReports": reports}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("artifacts", type=pathlib.Path, nargs="+")
    args = parser.parse_args()
    print(json.dumps(verify(*args.artifacts), indent=2) + "\n")
