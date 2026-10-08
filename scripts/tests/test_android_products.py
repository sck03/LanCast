import ast
from pathlib import Path
import re
import struct
import tempfile
from types import SimpleNamespace
import unittest
import zipfile

from scripts.android_artifacts import find_apks, verify_apk, verify_elf
from scripts.android_products import PRODUCTS, SELECTION_LABELS, gradle_tasks, selected_products

ROOT = Path(__file__).resolve().parents[2]
CONFIG = SimpleNamespace(version="1.2.3", build_number=17, android_abis=("armeabi-v7a", "arm64-v8a"))
SENDER_MANIFEST = '''E: uses-permission
  A: android:name(0x01010003)="android.permission.FOREGROUND_SERVICE_MEDIA_PROJECTION"
E: service
  A: android:name(0x01010003)="dev.lancast.sender.CaptureService"
  A: android:foregroundServiceType(0x01010599)=(type 0x11)0x20
'''


def elf(abi, alignment=None):
    wide = abi == "arm64-v8a"
    header, stride = (64, 56) if wide else (52, 32)
    data = bytearray(header + stride)
    data[:7] = b"\x7fELF" + bytes((2 if wide else 1, 1, 1))
    struct.pack_into("<HH", data, 16, 3, 183 if wide else 40)
    struct.pack_into("<Q" if wide else "<I", data, 32 if wide else 28, header)
    struct.pack_into("<HH", data, 54 if wide else 42, stride, 1)
    struct.pack_into("<I", data, header, 1)
    struct.pack_into("<Q" if wide else "<I", data, header + (48 if wide else 28), alignment or (16384 if wide else 4096))
    return data


def metadata(product):
    return (f"package: name='{product.application_id}' versionCode='17' versionName='1.2.3'\n"
            f"sdkVersion:'{product.minimum_sdk}'\napplication-label:'LanCast {product.name}'\n")


def make_apk(path, product, extra=None):
    with zipfile.ZipFile(path, "w") as archive:
        for abi in CONFIG.android_abis:
            libraries = ["liblancast_core.so", "libjingle_peerconnection_so.so"]
            if product.airplay:
                libraries.append("liblancast_airplay.so")
            if product.role == "sender":
                libraries.append("liblancast_media.so")
            for library in libraries:
                archive.writestr(f"lib/{abi}/{library}", elf(abi))
        if product.airplay:
            archive.writestr("assets/airplay/LICENSE.txt", "GPL-3.0-only")
        for name, data in (extra or {}).items():
            archive.writestr(name, data)


class AndroidProductsTests(unittest.TestCase):
    def test_selection_limits_tasks_and_native_features(self):
        sender = selected_products("sender", environ={})
        self.assertEqual([p.key for p in sender], ["sender"])
        self.assertEqual(gradle_tasks(sender, "Release"), [":app-sender:assembleRelease", ":app-sender:lintRelease", ":control-bridge:testReleaseUnitTest"])
        receivers = selected_products("receivers", environ={})
        self.assertEqual(len(receivers), 3)
        self.assertFalse(any("sender" in p.features for p in receivers))
        self.assertFalse(any(":app-sender:" in task for task in gradle_tasks(receivers, "Debug")))
        self.assertIn(":receiver-contracts:test", gradle_tasks(receivers, "Debug"))

    def test_actions_choices_and_cli_stay_in_sync(self):
        workflow = (ROOT / ".github/workflows/android.yml").read_text(encoding="utf-8")
        options = re.search(r"      product:.*?        options: (\[.*\])", workflow, re.DOTALL)[1].splitlines()[0]
        self.assertEqual(ast.literal_eval(options), list(SELECTION_LABELS.values()))
        for key, label in SELECTION_LABELS.items():
            self.assertEqual(selected_products(key, environ={}), selected_products(environ={"LC_ANDROID_PRODUCT": label}))
        with self.assertRaises(ValueError):
            selected_products("receiver-typo", environ={})
        with self.assertRaises(ValueError):
            selected_products("sender\nINJECT=1", environ={})

    def test_missing_product_is_not_hidden_by_four_other_apks(self):
        standard = PRODUCTS[1]
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            sender = PRODUCTS[0].apk_directory(root, "Debug")
            sender.mkdir(parents=True)
            for index in range(4):
                (sender / f"sender-{index}.apk").touch()
            with self.assertRaisesRegex(ValueError, "receiver-standard"):
                find_apks(root, (standard,), "Debug")
            target = standard.apk_directory(root, "Debug")
            target.mkdir(parents=True)
            (target / "standard.apk").touch()
            self.assertEqual(find_apks(root, (standard,), "Debug"), [(standard, target / "standard.apk")])
            (target / "stale.apk").touch()
            with self.assertRaisesRegex(ValueError, "found 2"):
                find_apks(root, (standard,), "Debug")

    def test_all_four_roles_verify_with_custom_version_inputs(self):
        with tempfile.TemporaryDirectory() as directory:
            for product in PRODUCTS:
                with self.subTest(product=product.key):
                    apk = Path(directory) / (product.key + ".apk")
                    make_apk(apk, product)
                    manifest = "dev.lancast.airplay.AirPlayService" if product.airplay else ""
                    if product.role == "sender":
                        manifest += SENDER_MANIFEST
                    report = verify_apk(apk, product, CONFIG, metadata(product), manifest)
                    self.assertEqual(report["role"], product.role)
                    self.assertEqual(len(report["sha256"]), 64)
                    with self.assertRaisesRegex(ValueError, "identity/version"):
                        verify_apk(apk, product, CONFIG, metadata(product).replace("1.2.3", "0.6.0"), manifest)

    def test_swapped_identity_capture_and_gpl_payloads_are_rejected(self):
        standard, legacy = PRODUCTS[1:3]
        with tempfile.TemporaryDirectory() as directory:
            apk = Path(directory) / "standard.apk"
            make_apk(apk, standard)
            for bad_metadata, manifest in ((metadata(legacy), ""), (metadata(standard), SENDER_MANIFEST),
                                           (metadata(standard), "dev.lancast.airplay.AirPlayService")):
                with self.assertRaises(ValueError):
                    verify_apk(apk, standard, CONFIG, bad_metadata, manifest)
            make_apk(apk, standard, {"lib/arm64-v8a/liblancast_airplay.so": elf("arm64-v8a")})
            with self.assertRaisesRegex(ValueError, "AirPlay native"):
                verify_apk(apk, standard, CONFIG, metadata(standard), "")

    def test_extra_native_libraries_also_require_page_alignment(self):
        standard = PRODUCTS[1]
        with tempfile.TemporaryDirectory() as directory:
            apk = Path(directory) / "standard.apk"
            make_apk(apk, standard, {"lib/arm64-v8a/libextra.so": elf("arm64-v8a", 4096)})
            with self.assertRaisesRegex(ValueError, "libextra.so.*alignment"):
                verify_apk(apk, standard, CONFIG, metadata(standard), "")
        with self.assertRaisesRegex(ValueError, "architecture"):
            malformed = elf("arm64-v8a")
            struct.pack_into("<H", malformed, 18, 62)
            verify_elf(malformed, "arm64-v8a")
        with self.assertRaisesRegex(ValueError, "program headers"):
            verify_elf(elf("arm64-v8a")[:-1], "arm64-v8a")


if __name__ == "__main__":
    unittest.main()
