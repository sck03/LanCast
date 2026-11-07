from pathlib import Path
import os
import json
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from scripts.workspace_cleanup import checked_path, clean, inventory, quarantine, snapshot


class WorkspaceCleanupTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        subprocess.run(["git", "init", "-q", str(self.root)], check=True)
        self.write(".gitignore", "target/\n.cache/\n**/__pycache__/\n.manual-delete/\n")

    def write(self, relative, content):
        path = self.root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")
        return path

    def test_preview_is_read_only_and_cleanup_preserves_source_and_unknown_cache(self):
        self.write("target/debug/program", "binary")
        self.write(".cache/artifacts/run/product.zip", "archive")
        self.write(".cache/hardware-capture.ts", "unique sample")
        self.write("src/main.rs", "source")
        before = snapshot(self.root)
        plan = inventory(self.root)
        self.assertEqual([entry["path"] for entry in plan], ["target"])
        self.assertEqual(snapshot(self.root), before)
        clean(self.root, plan)
        self.assertFalse((self.root / "target").exists())
        self.assertTrue((self.root / ".cache/artifacts/run/product.zip").exists())
        clean(self.root, inventory(self.root, include_downloads=True))
        self.assertTrue((self.root / ".cache/hardware-capture.ts").exists())
        self.assertTrue((self.root / "src/main.rs").exists())
        self.assertEqual(inventory(self.root, include_downloads=True), [])

    def test_tracked_files_block_the_entire_cleanup_plan(self):
        self.write("target/keep.rs", "tracked source")
        self.write("scripts/__pycache__/stale.pyc", "bytecode")
        subprocess.run(["git", "-C", str(self.root), "add", "-f", "target/keep.rs"], check=True)
        plan = inventory(self.root)
        with self.assertRaisesRegex(ValueError, "Tracked source"):
            clean(self.root, plan)
        self.assertTrue((self.root / "scripts/__pycache__/stale.pyc").exists())
        self.assertTrue((self.root / "target/keep.rs").exists())

    def test_changed_files_or_index_are_rechecked_before_deleting(self):
        path = self.write("target/program", "binary")
        plan = inventory(self.root)
        path.write_text("new compilation", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "Files changed"):
            clean(self.root, plan)
        plan = inventory(self.root)
        subprocess.run(["git", "-C", str(self.root), "add", "-f", "target/program"], check=True)
        with self.assertRaisesRegex(ValueError, "Tracked source"):
            clean(self.root, plan)
        self.assertTrue(path.exists())

    def test_nested_repositories_and_signing_material_are_protected(self):
        for name in (".git", "release.keystore", "signing.p12"):
            with self.subTest(name=name):
                path = self.write(f"target/{name}", "keep")
                with self.assertRaisesRegex(ValueError, "Protected content"):
                    clean(self.root, inventory(self.root))
                self.assertTrue(path.exists())
                path.unlink()

    def test_unignored_and_outside_paths_are_rejected(self):
        self.write("android/app/build/output", "not ignored")
        with self.assertRaisesRegex(ValueError, "Not an ignored"):
            clean(self.root, inventory(self.root))
        for relative in (".", "../outside", str(self.root)):
            with self.subTest(path=relative), self.assertRaises(ValueError):
                checked_path(self.root, relative)

    def test_directory_links_cannot_escape_or_hide_inside_a_candidate(self):
        with tempfile.TemporaryDirectory() as other:
            external = Path(other)
            (external / "keep").write_text("outside", encoding="utf-8")
            for relative in ("target", "target/nested"):
                path = self.root / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                if os.name == "nt":
                    subprocess.run(["cmd", "/c", "mklink", "/J", str(path), str(external)],
                                   capture_output=True, check=True)
                else:
                    path.symlink_to(external, target_is_directory=True)
                try:
                    with self.assertRaisesRegex(ValueError, "[Ll]ink|Protected content"):
                        clean(self.root, inventory(self.root))
                    self.assertEqual((external / "keep").read_text(encoding="utf-8"), "outside")
                finally:
                    path.rmdir() if os.name == "nt" else path.unlink()

    def test_quarantine_preserves_content_and_records_original_paths(self):
        self.write("target/debug/program", "binary")
        self.write(".cache/artifacts/run/product.zip", "archive")
        plan = inventory(self.root, include_downloads=True)
        destination = quarantine(self.root, plan)
        self.assertEqual((destination / "target/debug/program").read_text(), "binary")
        self.assertEqual((destination / ".cache/artifacts/run/product.zip").read_text(), "archive")
        manifest = json.loads((destination / "manifest.json").read_text())
        self.assertTrue(all(entry["moved"] for entry in manifest["paths"]))
        self.assertEqual([e["path"] for e in manifest["paths"]], [e["path"] for e in plan])
        self.assertEqual(inventory(self.root, include_downloads=True), [])
        self.assertIsNone(quarantine(self.root, []))
        # A new run must preserve previous quarantines.
        self.write("target/new", "replacement")
        second = quarantine(self.root, inventory(self.root))
        self.assertNotEqual(second, destination)
        self.assertTrue((destination / "target/debug/program").exists())

    def test_quarantine_rejects_changes_and_overlapping_plans_before_moving(self):
        path = self.write("target/program", "binary")
        plan = inventory(self.root)
        with self.assertRaisesRegex(ValueError, "Overlapping"):
            quarantine(self.root, plan + plan)
        path.write_text("changed", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "Files changed"):
            quarantine(self.root, plan)
        self.assertTrue(path.exists())
        self.assertFalse((self.root / ".manual-delete").exists())

    def test_quarantine_rejects_a_linked_destination(self):
        self.write("target/program", "binary")
        with tempfile.TemporaryDirectory() as other:
            path = self.root / ".manual-delete"
            if os.name == "nt":
                subprocess.run(["cmd", "/c", "mklink", "/J", str(path), other], capture_output=True, check=True)
            else:
                path.symlink_to(other, target_is_directory=True)
            try:
                with self.assertRaisesRegex(ValueError, "[Ll]ink"):
                    quarantine(self.root, inventory(self.root))
                self.assertTrue((self.root / "target/program").exists())
                self.assertEqual(list(Path(other).iterdir()), [])
            finally:
                path.rmdir() if os.name == "nt" else path.unlink()

    def test_quarantine_failure_preserves_unmoved_files_and_a_recovery_manifest(self):
        self.write("target/program", "binary")
        self.write("scripts/__pycache__/stale.pyc", "bytecode")
        rename = Path.rename

        def fail_second(source, target):
            if source.name == "__pycache__":
                raise PermissionError("file in use")
            return rename(source, target)

        with patch.object(Path, "rename", fail_second), self.assertRaises(PermissionError):
            quarantine(self.root, inventory(self.root))
        manifests = list((self.root / ".manual-delete").glob("*/manifest.json"))
        self.assertEqual(len(manifests), 1)
        manifest = json.loads(manifests[0].read_text())
        self.assertTrue(manifest["paths"][0]["moved"])
        self.assertFalse(manifest["paths"][1]["moved"])
        self.assertIn("file in use", manifest["paths"][1]["error"])
        self.assertEqual((manifests[0].parent / "target/program").read_text(), "binary")
        self.assertTrue((self.root / "scripts/__pycache__/stale.pyc").exists())
