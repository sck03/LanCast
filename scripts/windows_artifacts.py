"""Windows package validation independent of the CLI and Python optimization."""
import hashlib
import json
from pathlib import Path, PureWindowsPath
import re
import struct

if __package__:
    from .artifact_checks import require
else:
    from artifact_checks import require

BINARIES = ("LanCast.exe", "lancast_core.dll", "lancast_rtc.dll", "lancast_ts.dll")
REPORTS = {"SHA256.json", "binary-dependencies.json"}
ROOT_FILES = frozenset((*BINARIES, "build-windows.json", "LICENSE", "THIRD_PARTY.md",
                       "BUILD-STATUS.txt", "使用说明.txt"))
SYSTEM_DLLS = frozenset({
    "advapi32.dll", "avrt.dll", "bcrypt.dll", "bcryptprimitives.dll", "combase.dll",
    "comctl32.dll", "comdlg32.dll", "crypt32.dll", "d3d11.dll", "dbghelp.dll", "dxgi.dll",
    "gdi32.dll", "iphlpapi.dll", "kernel32.dll", "mf.dll", "mfplat.dll", "mfuuid.dll",
    "msvcrt.dll", "ncrypt.dll", "ntdll.dll", "ole32.dll", "oleaut32.dll", "powrprof.dll",
    "rpcrt4.dll", "runtimeobject.dll", "secur32.dll", "shell32.dll", "shlwapi.dll", "ucrtbase.dll",
    "user32.dll", "uuid.dll", "version.dll", "windowsapp.dll", "winmm.dll", "ws2_32.dll",
    "userenv.dll", "wintrust.dll", "wmcodecdspuuid.dll",
})
REQUIRED_LICENSES = frozenset({"FFmpeg-LGPL-2.1.txt", "libdatachannel-MPL-2.0.txt",
                               "opus-COPYING.txt", "mbedtls-LICENSE.txt", "json-MIT.txt"})
LICENSE_COMPONENTS = ("libjuice", "libsrtp", "usrsctp", "plog")


def package_files(root):
    root = Path(root)
    require(root.is_dir(), "Missing Windows package directory")
    entries = list(root.rglob("*"))
    require(not any(path.is_symlink() for path in entries), "Package links are not allowed")
    require(all(path.relative_to(root).as_posix() == "licenses" for path in entries if path.is_dir()),
            "Unexpected package directory")
    actual = {path.relative_to(root).as_posix() for path in entries if path.is_file()} - REPORTS
    require({path for path in actual if "/" not in path} == ROOT_FILES,
            "Missing or unexpected root package file")
    licenses = {path.removeprefix("licenses/") for path in actual if path.startswith("licenses/")}
    require(REQUIRED_LICENSES <= licenses, "Missing dependency licenses")
    require(all(name in REQUIRED_LICENSES or re.fullmatch(r"(?:libjuice|libsrtp|usrsctp|plog)-(?:LICENSE|COPYING|COPYRIGHT)[^/]*\.txt", name)
                for name in licenses), "Unexpected license file")
    for component in LICENSE_COMPONENTS:
        require(any(name.startswith(component + "-") for name in licenses),
                f"Missing dependency license: {component}")
    return actual


def write_hashes(root):
    root = Path(root)
    entries = [{"Path": name, "Hash": hashlib.sha256((root / name).read_bytes()).hexdigest()}
               for name in sorted(package_files(root))]
    (root / "SHA256.json").write_text(json.dumps(entries, indent=2) + "\n", encoding="utf-8")


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

    def import_name(rva):
        pointer = offset(rva)
        end = data.find(b"\0", pointer, min(pointer + 256, len(data)))
        require(end > pointer, f"Invalid import name: {name}")
        imported = data[pointer:end].decode("ascii").lower()
        require(re.fullmatch(r"[a-z0-9_.-]+\.dll", imported), f"Invalid imported DLL: {name}")
        require(not any(part in imported for part in ("msvcp", "vcruntime", "libstdc++", "libgcc", "libwinpthread")),
                f"Unshipped compiler runtime: {name} -> {imported}")
        imports.append(imported)

    for directory, stride, descriptor, name_field in ((1, 20, "<IIIII", 3), (13, 32, "<IIIIIIII", 1)):
        import_rva = read("<I", optional + 112 + directory * 8)[0]
        if not import_rva:
            continue
        for index in range(256):
            record = read(descriptor, offset(import_rva + index * stride, stride))
            if not any(record):
                break
            require(directory != 13 or record[0] == 1, f"Unsupported delay import address format: {name}")
            import_name(record[name_field])
        else:
            raise ValueError(f"Unterminated PE imports: {name}")
    return sorted(set(imports))


def verify(root):
    root = Path(root)
    actual = package_files(root)
    hashes = json.loads((root / "SHA256.json").read_text(encoding="utf-8-sig"))
    require(isinstance(hashes, list), "Invalid hash manifest")
    hashed = set()
    for entry in hashes:
        raw_name = str(entry.get("Path", "")).replace("\\", "/")
        name = PureWindowsPath(raw_name).as_posix()
        path = Path(name)
        require(name and not path.is_absolute() and ".." not in path.parts and name not in hashed and
                name not in REPORTS, f"Invalid hash entry: {name}")
        require(name in actual, f"Hash entry is not a package file: {name}")
        require(hashlib.sha256((root / path).read_bytes()).hexdigest() == entry["Hash"].lower(),
                f"Hash mismatch: {name}")
        hashed.add(name)
    require(hashed == actual, "Incomplete package hashes")
    config = json.loads((root / "build-windows.json").read_text(encoding="utf-8-sig"))
    require(config["configuration"] in ("Debug", "Release"), "Invalid Windows configuration")
    packaged = {name.lower() for name in BINARIES}
    report = []
    for name in BINARIES:
        data = (root / name).read_bytes()
        imports = verify_pe(data, name, config["configuration"])
        for imported in imports:
            require(imported in packaged or imported in SYSTEM_DLLS or
                    imported.startswith(("api-ms-win-", "ext-ms-win-")),
                    f"Unresolved packaged dependency: {name} -> {imported}")
        report.append({"file": name, "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest(),
                       "imports": imports})
    return report
