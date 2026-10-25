import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch
from scripts.build_config import check_recorded, record, resolve

DEFAULTS = {"schema": 1, "version": "0.4.0", "build_number": 4,
            "configuration": {p: "Release" for p in ("windows", "android", "macos", "ios", "tvos")},
            "android_abis": ["armeabi-v7a", "arm64-v8a"]}


class BuildConfigTests(unittest.TestCase):
    def test_packaging_cannot_replace_recorded_build_sources_or_configuration(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            subprocess.run(["git", "init", "--quiet", str(root)], check=True, capture_output=True)
            (root / ".gitignore").write_text("dist/\n", encoding="utf-8")
            (root / "source.txt").write_text("original", encoding="utf-8")
            subprocess.run(["git", "-C", str(root), "add", "."], check=True, capture_output=True)
            subprocess.run(["git", "-C", str(root), "-c", "user.name=Test", "-c", "user.email=test@example.invalid",
                            "commit", "-qm", "fixture"], check=True, capture_output=True)
            config = resolve("windows", environ={}, defaults=DEFAULTS)
            with patch("scripts.build_config.ROOT", root):
                original = record(config)
                self.assertEqual(check_recorded(config), json.loads(json.dumps(original)))
                with self.assertRaisesRegex(ValueError, "rebuild"):
                    check_recorded(resolve("windows", configuration="Debug", environ={}, defaults=DEFAULTS))
                (root / "source.txt").write_text("changed", encoding="utf-8")
                with self.assertRaisesRegex(ValueError, "rebuild"):
                    check_recorded(config)
            self.assertEqual((root / "dist/reports/build-windows.json").read_text(encoding="utf-8"),
                             json.dumps(original, indent=2) + "\n")

    def test_android_abi_cache_keys_are_safe_order_independent_and_distinct(self):
        dual = resolve("android", environ={}, defaults=DEFAULTS)
        reversed_abis = resolve("android", android_abis="arm64-v8a,armeabi-v7a", environ={}, defaults=DEFAULTS)
        self.assertEqual(dual.android_abi_key, reversed_abis.android_abi_key)
        keys = {dual.android_abi_key}
        for abi in DEFAULTS["android_abis"]:
            keys.add(resolve("android", android_abis=abi, environ={}, defaults=DEFAULTS).android_abi_key)
        self.assertEqual(len(keys), 3)
        for key in keys:
            self.assertRegex(key, r"^[a-z0-9-]+$")
        workflow = (Path(__file__).resolve().parents[2] / ".github/workflows/android.yml").read_text(encoding="utf-8")
        native_keys = [line for line in workflow.splitlines() if "key: android-" in line]
        self.assertTrue(native_keys)
        for line in native_keys:
            self.assertIn("${{ steps.build-config.outputs.android_abi_key }}", line)
            self.assertNotIn("${{ env.LC_ANDROID_ABIS }}", line)

    def test_chinese_workflow_choices_export_native_build_modes(self):
        for label, mode in (("日常使用（Release）", "Release"), ("开发调试（Debug）", "Debug")):
            for platform in DEFAULTS["configuration"]:
                with self.subTest(platform=platform, label=label):
                    config = resolve(platform, environ={"LC_CONFIGURATION": label}, defaults=DEFAULTS)
                    self.assertEqual(config.environment()["LC_CONFIGURATION"], mode)
                    self.assertEqual(config.label, f"0.4.0-4-{mode}")
        with self.assertRaises(ValueError):
            resolve("windows", configuration="日常使用（Release）\nINJECT=1", environ={}, defaults=DEFAULTS)

    def test_cli_environment_and_defaults_have_stable_precedence(self):
        c = resolve("windows", version="1.2.3", environ={"LC_VERSION": "9.0.0", "LC_BUILD_NUMBER": "27"}, defaults=DEFAULTS)
        self.assertEqual((c.version, c.build_number, c.configuration), ("1.2.3", 27, "Release"))
        c = resolve("android", environ={"LC_ANDROID_ABIS": "arm64-v8a"}, defaults=DEFAULTS)
        self.assertEqual(c.android_abis, ("arm64-v8a",))

    def test_incompatible_versions_and_workflow_injection_are_rejected(self):
        for value in ("1.2", "1.2.3-beta", "01.2.3", "1.2.3\nLC_BUILD_NUMBER=1", "$(id)", "1.2.10000"):
            with self.subTest(version=value), self.assertRaises(ValueError):
                resolve("ios", version=value, environ={}, defaults=DEFAULTS)
        for value in ("0", "-1", "65536", "01", "1\nINJECT=1"):
            with self.subTest(build=value), self.assertRaises(ValueError):
                resolve("windows", build_number=value, environ={}, defaults=DEFAULTS)
        for value in ("x86_64", "arm64-v8a,arm64-v8a", "arm64-v8a,", "armeabi-v7a\n"):
            with self.subTest(abis=value), self.assertRaises(ValueError):
                resolve("android", android_abis=value, environ={}, defaults=DEFAULTS)


if __name__ == "__main__":
    unittest.main()
