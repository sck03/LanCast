import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

SCRIPTS = Path(__file__).resolve().parents[1]
with patch.object(sys, "path", [str(SCRIPTS), *sys.path]):
    SPEC = importlib.util.spec_from_file_location("verify_apple", SCRIPTS / "verify-apple-artifacts.py")
    VERIFIER = importlib.util.module_from_spec(SPEC)
    SPEC.loader.exec_module(VERIFIER)


class AppleArtifactPathsTests(unittest.TestCase):
    def test_missing_requested_download_cannot_be_masked_by_another_platform(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            tvos = root / "tvos"
            tvos.mkdir()
            (tvos / "build-report.json").write_text(json.dumps({"platform": "tvos"}), encoding="utf-8")
            self.assertEqual(VERIFIER.find_builds((tvos,)), {"tvos": tvos})
            with self.assertRaisesRegex(AssertionError, "Missing artifact directory"):
                VERIFIER.find_builds((tvos, root / "unfinished-macos-download"))
            empty = root / "empty"
            empty.mkdir()
            with self.assertRaisesRegex(AssertionError, "No Apple build report in requested"):
                VERIFIER.find_builds((tvos, empty))

    def test_duplicate_platform_reports_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            roots = [Path(directory) / "user-package", Path(directory) / "diagnostics"]
            for root in roots:
                root.mkdir()
                (root / "build-report.json").write_text(json.dumps({"platform": "tvos"}), encoding="utf-8")
            with self.assertRaisesRegex(AssertionError, "Multiple builds for one platform"):
                VERIFIER.find_builds(roots)


if __name__ == "__main__":
    unittest.main()
