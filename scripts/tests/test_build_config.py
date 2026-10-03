import unittest
from scripts.build_config import resolve

DEFAULTS = {"schema": 1, "version": "0.4.0", "build_number": 4,
            "configuration": {p: "Release" for p in ("windows", "android", "macos", "ios", "tvos")},
            "android_abis": ["armeabi-v7a", "arm64-v8a"]}


class BuildConfigTests(unittest.TestCase):
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
