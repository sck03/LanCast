"""Windows package validation independent of the CLI and Python optimization."""
import hashlib
import json
from pathlib import Path, PureWindowsPath
import struct

if __package__:
    from .artifact_checks import require
else:
    from artifact_checks import require

BINARIES = ("LanCast.exe", "lancast_core.dll", "lancast_rtc.dll", "lancast_ts.dll")
REPORTS = {"SHA256.json", "binary-dependencies.json"}


def verify_pe(data, name, configuration):
    def read(fmt, offset):
        require(0 <= offset <= len(data) - struct.calcsize(fmt), f"Truncated PE: {name}")
        return struct.unpack_from(fmt, data, offset)

    require(len(data) >= 64 and data[:2] == b"MZ", f"Invalid PE: {name}")
    pe = read("<I", 0x3C)[0]
    require(pe >= 64 and data[pe:pe + 4] == b"PE\0\0", f"Invalid PE signature: {name}")
    machine, sections = read("<HH", pe + 4)
    require(machine == 0x8664 and 0 < sections <= 96, f"Invalid x64 PE sections: {name}")
    optional_size = read("<H", pe + 20)[0]
    optional = pe + 24
    require(optional_size >= 240 and read("<H", optional)[0] == 0x20B, f"Invalid PE64 header: {name}")
    table = optional + optional_size
    require(table + sections * 40 <= len(data), f"Truncated section table: {name}")
    if name == "lancast_ts.dll" and configuration == "Release":
        require(read("<II", pe + 12) == (0, 0), "Release TS DLL contains a COFF symbol table")
        require(all(not data[table + i * 40:table + i * 40 + 8].startswith((b".debug", b"/"))
                    for i in range(sections)), "Release TS DLL contains debug sections")

    def offset(rva, size=1):
        for index in range(sections):
            virtual_size, virtual, raw_size, raw = read("<IIII", table + index * 40 + 8)
            if virtual <= rva < virtual + max(virtual_size, raw_size):
                relative = rva - virtual
                require(relative + size <= raw_size and raw + relative + size <= len(data),
                        f"Invalid raw RVA: {name}/{rva}")
                return raw + relative
        raise ValueError(f"Invalid RVA: {name}/{rva}")

    imports = []
    import_rva = read("<I", optional + 120)[0]
    if import_rva:
        for index in range(256):
            record = read("<IIIII", offset(import_rva + index * 20, 20))
            if not any(record):
                break
            pointer = offset(record[3])
            end = data.find(b"\0", pointer, min(pointer + 256, len(data)))
            require(end > pointer, f"Invalid import name: {name}")
            imported = data[pointer:end].decode("ascii").lower()
            require(not any(part in imported for part in ("msvcp", "vcruntime", "libstdc++", "libgcc", "libwinpthread")),
                    f"Unshipped compiler runtime: {name} -> {imported}")
            imports.append(imported)
        else:
            raise ValueError(f"Unterminated PE imports: {name}")
    return imports


def verify(root):
    root = Path(root)
    hashes = json.loads((root / "SHA256.json").read_text(encoding="utf-8-sig"))
    require(isinstance(hashes, list), "Invalid hash manifest")
    hashed = set()
    for entry in hashes:
        name = PureWindowsPath(entry["Path"]).name
        require(name and name not in hashed and name not in REPORTS, f"Invalid hash entry: {name}")
        require(hashlib.sha256((root / name).read_bytes()).hexdigest() == entry["Hash"].lower(), f"Hash mismatch: {name}")
        hashed.add(name)
    require(hashed == {path.name for path in root.iterdir() if path.is_file()} - REPORTS, "Incomplete package hashes")
    require(set(BINARIES) <= hashed, "Missing Windows binaries")
    config = json.loads((root / "build-windows.json").read_text(encoding="utf-8-sig"))
    require(config["configuration"] in ("Debug", "Release"), "Invalid Windows configuration")
    report = []
    for name in BINARIES:
        data = (root / name).read_bytes()
        report.append({"file": name, "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest(),
                       "imports": verify_pe(data, name, config["configuration"])})
    return report
