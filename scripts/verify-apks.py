#!/usr/bin/env python3
"""Fail CI when APKs omit JNI ABIs or mix unexpected player dependencies."""
import pathlib, struct, zipfile, os, re, subprocess, json, hashlib
from build_config import resolve, record
root = pathlib.Path(__file__).resolve().parents[1]
config = resolve("android")
mode = config.configuration.lower()
apks = list((root/"android").glob(f"app-*/build/outputs/apk/**/{mode}/*.apk"))
assert len(apks) == 4, f"Expected 4 {mode} APKs, found {len(apks)}"
sdk = pathlib.Path(os.environ.get("ANDROID_HOME") or os.environ["ANDROID_SDK_ROOT"])
aapt = sdk / "build-tools/35.0.0" / ("aapt.exe" if os.name == "nt" else "aapt")
report = record(config)
report["artifacts"] = []
for apk in apks:
    metadata = subprocess.check_output([str(aapt), "dump", "badging", str(apk)], text=True)
    assert re.search(r"versionCode='" + str(config.build_number) + "'", metadata), "APK build number mismatch"
    assert re.search(r"versionName='" + re.escape(config.version) + "'", metadata), "APK version mismatch"
    with zipfile.ZipFile(apk) as archive:
        packaged_abis = {p.split("/")[1] for p in archive.namelist() if p.startswith("lib/") and p.endswith(".so")}
        assert packaged_abis == set(config.android_abis), f"Unexpected ABIs: {packaged_abis}"
        for abi in config.android_abis:
            libraries = ["liblancast_core.so", "libjingle_peerconnection_so.so"]
            if "airplay" in apk.parts:
                libraries.append("liblancast_airplay.so")
            else:
                assert f"lib/{abi}/liblancast_airplay.so" not in archive.namelist(), "GPL AirPlay library leaked into a base product"
            if "app-sender" in apk.parts:
                libraries.append("liblancast_media.so")
            else:
                assert f"lib/{abi}/liblancast_media.so" not in archive.namelist(), "TS sender library leaked into receiver"
            for lib in libraries:
                path = f"lib/{abi}/{lib}"
                assert path in archive.namelist(), f"{apk.name}: missing {path}"
                elf = archive.read(path)
                assert elf[:4] == b"\x7fELF"
                # Verify every PT_LOAD alignment, rather than trusting a linker flag.
                bits, endian = elf[4], "<" if elf[5] == 1 else ">"
                if bits == 2:
                    offset = struct.unpack_from(endian+"Q", elf, 32)[0]
                    size, count = struct.unpack_from(endian+"HH",elf,54)
                else:
                    offset = struct.unpack_from(endian+"I",elf,28)[0]
                    size, count = struct.unpack_from(endian+"HH",elf,42)
                for n in range(count):
                    base = offset + n*size
                    kind = struct.unpack_from(endian+"I",elf,base)[0]
                    if kind == 1:
                        align = struct.unpack_from(endian+("Q" if bits==2 else "I"),elf,base+(48 if bits==2 else 28))[0]
                        # Android's 16KB execution target is ARM64. The separate ARM32
                        # legacy ABI is a 4KB target; never label its upstream library 16KB.
                        required = 16384 if abi == "arm64-v8a" else 4096
                        assert align >= required, f"{apk.name}/{path}: PT_LOAD alignment {align} is below {required}"
    print(f"PASS {apk.name}: expected JNI, ARM64 16KB and ARM32 4KB PT_LOAD alignment")
    report["artifacts"].append({"file": str(apk.relative_to(root)), "bytes": apk.stat().st_size, "sha256": hashlib.sha256(apk.read_bytes()).hexdigest()})
(root / "dist/reports/build-android.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
