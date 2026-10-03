#!/usr/bin/env python3
"""Inspect PE imports so a development machine cannot hide unshipped toolchain DLLs."""
import hashlib
import json
from pathlib import Path
import struct

root = Path(__file__).resolve().parents[1] / "dist/LanCast-Windows-x64"
expected = ["LanCast.exe", "lancast_core.dll", "lancast_rtc.dll", "lancast_ts.dll"]
report = []
for name in expected:
    data = (root / name).read_bytes()
    assert data[:2] == b"MZ", f"Invalid PE: {name}"
    pe = struct.unpack_from("<I", data, 0x3C)[0]
    assert data[pe:pe+4] == b"PE\0\0" and struct.unpack_from("<H", data, pe+4)[0] == 0x8664
    sections = struct.unpack_from("<H", data, pe+6)[0]
    optional_size = struct.unpack_from("<H", data, pe+20)[0]
    optional = pe + 24
    assert struct.unpack_from("<H", data, optional)[0] == 0x20B
    table = optional + optional_size

    def offset(rva):
        for index in range(sections):
            base = table + index * 40
            virtual_size, virtual, raw_size, raw = struct.unpack_from("<IIII", data, base+8)
            if virtual <= rva < virtual + max(virtual_size, raw_size):
                result = raw + rva - virtual
                assert result < len(data)
                return result
        raise AssertionError(f"Invalid RVA: {name}/{rva}")

    imports = []
    import_rva = struct.unpack_from("<I", data, optional+120)[0]
    if import_rva:
        start = offset(import_rva)
        for index in range(256):
            record = struct.unpack_from("<IIIII", data, start+index*20)
            if not any(record):
                break
            pointer = offset(record[3])
            end = data.index(b"\0", pointer, pointer+256)
            imported = data[pointer:end].decode("ascii").lower()
            assert not any(part in imported for part in ("msvcp", "vcruntime", "libstdc++", "libgcc", "libwinpthread")), f"Unshipped compiler runtime: {name} -> {imported}"
            imports.append(imported)
    report.append({"file": name, "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest(), "imports": imports})
(root / "binary-dependencies.json").write_text(json.dumps(report, indent=2), encoding="utf-8")
print("PASS: x64 PE imports, no external VC/GCC runtime, binary hashes recorded")
