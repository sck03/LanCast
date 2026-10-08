"""Verify Android product identity and native payloads before packaging user downloads."""
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import struct
import subprocess
import zipfile


def require(condition, message):
    if not condition:
        raise ValueError(message)


def verify_elf(data, abi):
    """Validate the actual architecture and every load segment, including extra JNI libraries."""
    bits, machine, required = (2, 183, 16384) if abi == "arm64-v8a" else (1, 40, 4096)
    require(len(data) >= 64 and data[:6] == b"\x7fELF" + bytes((bits, 1)), "Invalid ELF class or byte order")
    require(struct.unpack_from("<HH", data, 16) == (3, machine), "Wrong ELF type or architecture")
    wide = bits == 2
    offset = struct.unpack_from("<Q" if wide else "<I", data, 32 if wide else 28)[0]
    stride, count = struct.unpack_from("<HH", data, 54 if wide else 42)
    require(stride == (56 if wide else 32) and count > 0 and offset + stride * count <= len(data), "Invalid ELF program headers")
    loads = 0
    for index in range(count):
        base = offset + index * stride
        if struct.unpack_from("<I", data, base)[0] != 1:
            continue
        loads += 1
        align = struct.unpack_from("<Q" if wide else "<I", data, base + (48 if wide else 28))[0]
        require(align >= required and align & (align - 1) == 0, f"PT_LOAD alignment {align} is below {required} or invalid")
    require(loads > 0, "ELF has no load segments")


def find_apks(root, products, mode):
    result = []
    for product in products:
        candidates = sorted(product.apk_directory(root, mode).glob("*.apk"))
        require(len(candidates) == 1, f"Expected one {product.key} {mode} APK, found {len(candidates)}")
        result.append((product, candidates[0]))
    return result


def verify_apk(apk, product, config, metadata, manifest):
    expected = {"name": product.application_id, "versionCode": str(config.build_number), "versionName": config.version}
    package = re.search(r"^package: (.+)$", metadata, re.MULTILINE)
    fields = dict(re.findall(r"(\w+)='([^']*)'", package[1])) if package else {}
    require(all(fields.get(key) == value for key, value in expected.items()), f"{product.key}: APK identity/version mismatch")
    require(f"sdkVersion:'{product.minimum_sdk}'" in metadata.splitlines(), f"{product.key}: minimum SDK mismatch")
    require(f"application-label:'LanCast {product.name}'" in metadata.splitlines(), f"{product.key}: application label mismatch")
    require(("dev.lancast.airplay.AirPlayService" in manifest) == product.airplay, "AirPlay service isolation failed")
    # aapt prints foregroundServiceType as a numeric flag, not "mediaProjection".
    capture = re.search(r'"(?:dev\.lancast\.sender)?\.CaptureService"', manifest) is not None
    require(capture == (product.role == "sender"), "Capture service role mismatch")
    require(("android.permission.FOREGROUND_SERVICE_MEDIA_PROJECTION" in manifest) == (product.role == "sender"),
            "Capture permission role mismatch")
    require("BOOT_COMPLETED" not in manifest, "Products must never start capture or AirPlay on boot")
    with zipfile.ZipFile(apk) as archive:
        names = archive.namelist()
        require(len(names) == len(set(names)), "Duplicate APK entries")
        has_notice = "assets/airplay/LICENSE.txt" in names
        require(has_notice == product.airplay, "AirPlay license asset isolation failed")
        libraries = [name for name in names if name.startswith("lib/") and name.endswith(".so")]
        require({name.split("/")[1] for name in libraries} == set(config.android_abis), "Packaged ABI mismatch")
        for abi in config.android_abis:
            packaged = {name.removeprefix(f"lib/{abi}/") for name in libraries if name.startswith(f"lib/{abi}/")}
            require({"liblancast_core.so", "libjingle_peerconnection_so.so"} <= packaged, "Missing control or RTC library")
            require(("liblancast_airplay.so" in packaged) == product.airplay, "AirPlay native library isolation failed")
            require(("liblancast_media.so" in packaged) == (product.role == "sender"), "Sender TS library isolation failed")
        for name in libraries:
            try:
                verify_elf(archive.read(name), name.split("/")[1])
            except ValueError as error:
                raise ValueError(f"{apk.name}/{name}: {error}") from error
    with apk.open("rb") as stream:
        digest = hashlib.file_digest(stream, "sha256").hexdigest()
    return {"product": product.key, "role": product.role, "application_id": product.application_id,
            "minimum_sdk": product.minimum_sdk, "abis": list(config.android_abis),
            "license": "GPL-3.0-only" if product.airplay else "Apache-2.0 with third-party notices",
            "bytes": apk.stat().st_size, "sha256": digest}


def verify_and_package(root, config, products, report):
    root = Path(root)
    sdk = os.environ.get("ANDROID_HOME") or os.environ.get("ANDROID_SDK_ROOT")
    require(sdk, "Set ANDROID_HOME or ANDROID_SDK_ROOT for APK verification")
    aapt = Path(sdk) / "build-tools/35.0.0" / ("aapt.exe" if os.name == "nt" else "aapt")
    verified = []
    for product, apk in find_apks(root, products, config.configuration):
        metadata = subprocess.check_output([str(aapt), "dump", "badging", str(apk)], text=True, encoding="utf-8")
        manifest = subprocess.check_output([str(aapt), "dump", "xmltree", str(apk), "AndroidManifest.xml"], text=True, encoding="utf-8")
        entry = verify_apk(apk, product, config, metadata, manifest)
        entry["file"] = apk.relative_to(root).as_posix()
        verified.append((product, apk, entry))
    report["products"] = [p.key for p in products]
    report["artifacts"] = []
    signing = "Debug development signature; upgrades require the same signing key" if config.configuration == "Debug" else "Unsigned Release APK; sign before installation"
    for product, apk, entry in verified:
        output = root / "dist/android" / config.label / product.key
        output.mkdir(parents=True, exist_ok=True)
        destination = output / f"{product.artifact_prefix}-{config.label}.apk"
        shutil.copy2(apk, destination)
        entry["package_file"] = destination.relative_to(root).as_posix()
        entry["signing"] = signing
        report["artifacts"].append(entry)
        (output / "manifest.json").write_text(json.dumps({"source_commit": report["source_commit"],
            "version": config.version, "build_number": config.build_number, "configuration": config.configuration,
            **entry, "file": destination.name}, indent=2) + "\n", encoding="utf-8")
        (output / "INSTALL.txt").write_text(
            f"LanCast {product.name}\nRole: {product.role}\nMinimum Android API: {product.minimum_sdk}\n"
            f"Application ID: {product.application_id}\n{signing}\n"
            f"Source commit: {report['source_commit']}\nLicense: {entry['license']}\n"
            "Receiver variants are alternatives; install the one matching your device and needs.\n"
            "Build and package checks do not replace physical phone/TV acceptance.\n"
            "Source/relink materials and build reports are separate artifacts in this Actions run.\n", encoding="utf-8")
        for name in ("LICENSE", "THIRD_PARTY.md"):
            shutil.copy2(root / name, output / name)
        if product.airplay:
            shutil.copy2(root / "airplay-native/LICENSE", output / "AirPlay-GPL-3.0.txt")
        print(f"PASS {product.key}: {destination.name}")
    (root / "dist/reports/build-android.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    if summary := os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(summary, "a", encoding="utf-8") as stream:
            stream.write(f"## Android downloads\n\nSource: `{report['source_commit']}` · {config.label}\n\n")
            stream.write("| Artifact | Role | Minimum API | APK bytes |\n|---|---|---|---|\n")
            for product, _, entry in verified:
                stream.write(f"| {product.artifact_prefix}-{config.label} | {product.role} | {product.minimum_sdk} | {entry['bytes']:,} |\n")
            stream.write(f"\n{signing}. Each APK contains all selected ABIs. Receiver variants are alternatives.\n")
    return report
