#!/usr/bin/env python3
"""Verify package hashes, PE imports and release TS symbol stripping."""
import argparse
import hashlib
import json
from pathlib import Path, PureWindowsPath
import struct

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("root", nargs="?", type=Path,
                    default=Path(__file__).resolve().parents[1] / "dist/LanCast-Windows-x64",
                    help="Extracted Windows application directory")
root = parser.parse_args().root
expected = ["LanCast.exe", "lancast_core.dll", "lancast_rtc.dll", "lancast_ts.dll"]
hashes = json.loads((root / "SHA256.json").read_text(encoding="utf-8-sig"))
hashed = set()
for entry in hashes:
    name = PureWindowsPath(entry["Path"]).name
    assert name not in hashed and name not in {"SHA256.json", "binary-dependencies.json"}, f"Invalid hash entry: {name}"
    assert hashlib.sha256((root / name).read_bytes()).hexdigest() == entry["Hash"].lower(), f"Hash mismatch: {name}"
    hashed.add(name)
assert hashed == {path.name for path in root.iterdir() if path.is_file()} - {"SHA256.json", "binary-dependencies.json"}, "Incomplete package hashes"
config = json.loads((root / "build-windows.json").read_text(encoding="utf-8-sig"))
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
    if name == "lancast_ts.dll" and config["configuration"] == "Release":
        assert struct.unpack_from("<II", data, pe+12) == (0, 0), "Release TS DLL contains a COFF symbol table"
        assert all(not data[table+i*40:table+i*40+8].startswith((b".debug", b"/"))
                   for i in range(sections)), "Release TS DLL contains debug sections"

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
print("PASS: package SHA-256, x64 PE imports, runtime dependencies and Release TS symbol policy")
