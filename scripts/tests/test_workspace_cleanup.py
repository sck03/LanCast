from pathlib import Path
import os
import subprocess
import tempfile
import unittest

from scripts.workspace_cleanup import checked_path, clean, inventory, snapshot


class WorkspaceCleanupTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        subprocess.run(["git", "init", "-q", str(self.root)], check=True)
        self.write(".gitignore", "target/\n.cache/\n**/__pycache__/\n")

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
