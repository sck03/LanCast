import json
from pathlib import Path
import subprocess
import tempfile
import unittest

from scripts.source_provenance import require_clean_source, source_state


class SourceProvenanceTests(unittest.TestCase):
    def test_dirty_sources_are_fingerprinted_but_cannot_be_archived_as_head(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)

            def git(*args):
                return subprocess.run(["git", *args], cwd=root, check=True, capture_output=True)

            git("init", "--quiet")
            (root / "source.txt").write_text("original", encoding="utf-8")
            git("add", ".")
            git("-c", "user.name=Test", "-c", "user.email=test@example.invalid", "commit", "-qm", "fixture")
            clean = require_clean_source(root)
            report = root / ".git/build.json"
            report.write_text(json.dumps(clean), encoding="utf-8")
            self.assertEqual(require_clean_source(root, report), clean)
            (root / "source.txt").write_text("modified", encoding="utf-8")
            changed = source_state(root)
            self.assertTrue(changed["source_dirty"])
            self.assertEqual(changed["source_commit"], clean["source_commit"])
            self.assertNotEqual(changed["source_tree_sha256"], clean["source_tree_sha256"])
            with self.assertRaisesRegex(ValueError, "committed sources"):
                require_clean_source(root, report)
            git("add", ".")
            git("-c", "user.name=Test", "-c", "user.email=test@example.invalid", "commit", "-qm", "changed fixture")
            with self.assertRaisesRegex(ValueError, "rebuild"):
                require_clean_source(root, report)

    def test_untracked_source_changes_are_part_of_local_build_identity(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            subprocess.run(["git", "init", "--quiet", str(root)], check=True, capture_output=True)
            subprocess.run(["git", "-C", str(root), "-c", "user.name=Test", "-c", "user.email=test@example.invalid",
                            "commit", "--allow-empty", "-qm", "fixture"], check=True, capture_output=True)
            clean = source_state(root)
            (root / "new-source.txt").write_text("new", encoding="utf-8")
            dirty = source_state(root)
            self.assertTrue(dirty["source_dirty"])
            self.assertNotEqual(dirty["source_tree_sha256"], clean["source_tree_sha256"])
