from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import unittest

SCRIPTS = Path(__file__).resolve().parents[1]
from scripts.windows_artifacts import BINARIES, LICENSE_COMPONENTS, REQUIRED_LICENSES, ROOT_FILES, verify, verify_pe, write_hashes


def pe_fixture():
    data = bytearray(1024)
    data[:2] = b"MZ"
    struct.pack_into("<I", data, 0x3C, 128)
    data[128:132] = b"PE\0\0"
    struct.pack_into("<HH", data, 132, 0x8664, 1)
    struct.pack_into("<H", data, 148, 240)
    struct.pack_into("<H", data, 152, 0x20B)
    table = 392
    data[table:table + 8] = b".text\0\0\0"
    struct.pack_into("<IIII", data, table + 8, 512, 0x1000, 512, 512)
    return data


def make_package(root):
    for name in ROOT_FILES:
        (root / name).write_bytes(pe_fixture() if name in BINARIES else b"notice")
    (root / "build-windows.json").write_text('{"configuration":"Release"}', encoding="utf-8")
    (root / "licenses").mkdir()
    for name in (*REQUIRED_LICENSES, *(component + "-LICENSE.txt" for component in LICENSE_COMPONENTS)):
        (root / "licenses" / name).write_text("notice", encoding="utf-8")
    write_hashes(root)


def with_import(name, delayed=False):
    data = pe_fixture()
    struct.pack_into("<I", data, 152 + 112 + (13 if delayed else 1) * 8, 0x1000)
    if delayed:
        struct.pack_into("<IIIIIIII", data, 512, 1, 0x1080, 0, 0, 0, 0, 0, 0)
    else:
        struct.pack_into("<IIIII", data, 512, 0, 0, 0, 0x1080, 0)
    data[640:640 + len(name) + 1] = name.encode("ascii") + b"\0"
    return data


class WindowsArtifactTests(unittest.TestCase):
    def test_valid_package_and_hash_rejection(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            make_package(root)
            self.assertEqual(len(verify(root)), 4)
            (root / BINARIES[0]).write_bytes(b"changed")
            with self.assertRaisesRegex(ValueError, "Hash mismatch"):
                verify(root)

    def test_stale_files_and_unhashed_notices_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            make_package(root)
            (root / "obsolete-helper.exe").write_bytes(b"stale")
            with self.assertRaisesRegex(ValueError, "unexpected root"):
                write_hashes(root)
            (root / "obsolete-helper.exe").unlink()
            notice = root / "licenses" / next(iter(REQUIRED_LICENSES))
            notice.write_text("changed", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "Hash mismatch"):
                verify(root)
            (root / "licenses/libjuice-LICENSE.txt").unlink()
            with self.assertRaisesRegex(ValueError, "Missing dependency license: libjuice"):
                verify(root)

    def test_direct_and_delay_imports_require_a_packaged_or_system_dll(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            make_package(root)
            for delayed in (False, True):
                for dependency in ("kernel32.dll", "lancast_rtc.dll", "api-ms-win-core-synch-l1-2-0.dll"):
                    (root / "LanCast.exe").write_bytes(with_import(dependency, delayed))
                    write_hashes(root)
                    self.assertIn(dependency, verify(root)[0]["imports"])
                (root / "LanCast.exe").write_bytes(with_import("missing_dependency.dll", delayed))
                write_hashes(root)
                with self.assertRaisesRegex(ValueError, "Unresolved packaged dependency"):
                    verify(root)

    def test_pe_architecture_bounds_and_release_symbols(self):
        valid = pe_fixture()
        self.assertEqual(verify_pe(valid, "LanCast.exe", "Release"), [])
        invalid = bytearray(valid)
        struct.pack_into("<H", invalid, 132, 0x14C)
        with self.assertRaisesRegex(ValueError, "x64"):
            verify_pe(invalid, "LanCast.exe", "Release")
        with self.assertRaises(ValueError):
            verify_pe(valid[:100], "LanCast.exe", "Release")
        invalid = bytearray(valid)
        struct.pack_into("<I", invalid, 140, 1)
        with self.assertRaisesRegex(ValueError, "COFF"):
            verify_pe(invalid, "lancast_ts.dll", "Release")

    def test_invalid_package_is_rejected_with_python_optimization(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in BINARIES:
                (root / name).write_bytes(bytes(512))
            (root / "SHA256.json").write_text("[]", encoding="utf-8")
            (root / "build-windows.json").write_text('{"configuration":"Release"}', encoding="utf-8")
            for flags in ([], ["-O"]):
                result = subprocess.run([sys.executable, "-B", *flags, str(SCRIPTS / "verify-windows.py"), str(root)],
                                        capture_output=True, text=True)
                self.assertNotEqual(result.returncode, 0)
                self.assertNotIn("PASS:", result.stdout)

    def test_apple_magic_validation_survives_optimization(self):
        program = ("import runpy,sys; sys.path.insert(0,sys.argv[1]); "
                   "module=runpy.run_path(sys.argv[1]+'/verify-apple-artifacts.py'); "
                   "module['architectures'](bytes(512))")
        for flags in ([], ["-O"]):
            result = subprocess.run([sys.executable, "-B", *flags, "-c", program, str(SCRIPTS)],
                                    capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("Expected a 64-bit Mach-O executable", result.stderr)
