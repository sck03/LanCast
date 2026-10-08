from pathlib import Path
import tempfile
import unittest
from scripts.apple_products import PRODUCTS, app_paths


class AppleProductsTests(unittest.TestCase):
    def test_tvos_remains_receiver_only(self):
        product = PRODUCTS["tvos"]
        self.assertEqual(product.role, "receiver")
        self.assertEqual(product.rust_features, "legacy")
        self.assertEqual(product.bundle_id, "dev.lancast.tv")
        self.assertIn("-Receiver-Device-", product.archive_name("Device", "0.6.1-9-Debug"))
        self.assertIn("-Receiver-Simulator-", product.archive_name("Simulator", "0.6.1-9-Debug"))

    def test_delivery_ignores_stale_configuration_and_unexpected_sdks(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in ("Debug-appletvos", "Debug-appletvsimulator", "Debug-old", "Release-appletvos", "Debug-iphoneos"):
                (root / name / "LanCastTV.app").mkdir(parents=True)
            selected = app_paths(root, "tvos", "Debug")
            self.assertEqual([(p.parent.name, kind) for p, kind in selected], [
                ("Debug-appletvos", "Device"), ("Debug-appletvsimulator", "Simulator")])
            (root / "Debug-appletvsimulator/LanCastTV.app").rmdir()
            self.assertFalse(app_paths(root, "tvos", "Debug")[1][0].exists())

    def test_platforms_have_explicit_delivery_targets(self):
        self.assertEqual(PRODUCTS["macos"].directories("Release"), (("Release", "Universal"),))
        self.assertEqual(PRODUCTS["ios"].directories("Debug"), (("Debug-iphoneos", "Device"), ("Debug-iphonesimulator", "Simulator")))


if __name__ == "__main__":
    unittest.main()
