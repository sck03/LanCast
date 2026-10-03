#!/usr/bin/env python3
"""Fail CI when APKs omit JNI ABIs or mix unexpected player dependencies."""
import pathlib, struct, zipfile
root = pathlib.Path(__file__).resolve().parents[1]
apks = list((root/"android").glob("app-*/build/outputs/apk/**/*debug.apk"))
assert len(apks) == 3, f"Expected 3 debug APKs, found {len(apks)}"
for apk in apks:
    with zipfile.ZipFile(apk) as archive:
        for abi in ("armeabi-v7a","arm64-v8a"):
            libraries = ["liblancast_core.so", "libjingle_peerconnection_so.so"]
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
