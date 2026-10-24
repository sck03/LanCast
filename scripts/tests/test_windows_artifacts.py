import hashlib
import json
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import unittest

SCRIPTS = Path(__file__).resolve().parents[1]
from scripts.windows_artifacts import BINARIES, verify, verify_pe


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


class WindowsArtifactTests(unittest.TestCase):
    def test_valid_package_and_hash_rejection(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in BINARIES:
                (root / name).write_bytes(pe_fixture())
            (root / "build-windows.json").write_text('{"configuration":"Release"}', encoding="utf-8")
            entries = [{"Path": file.name, "Hash": hashlib.sha256(file.read_bytes()).hexdigest()}
                       for file in root.iterdir()]
            (root / "SHA256.json").write_text(json.dumps(entries), encoding="utf-8")
            self.assertEqual(len(verify(root)), 4)
            (root / BINARIES[0]).write_bytes(b"changed")
            with self.assertRaisesRegex(ValueError, "Hash mismatch"):
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
