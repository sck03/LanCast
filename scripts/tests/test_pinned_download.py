import hashlib
from pathlib import Path
import tempfile
import tracemalloc
import unittest
from unittest.mock import patch

from scripts.pinned_download import fetch, sha256_file


class PinnedDownloadTests(unittest.TestCase):
    def test_cache_repair_verification_and_reuse(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source, cached = root / "source.zip", root / "cached.zip"
            source.write_bytes(b"verified input")
            digest = sha256_file(source)
            cached.write_bytes(b"interrupted old download")
            fetch(source.as_uri(), cached, digest)
            self.assertEqual(sha256_file(cached), digest)
            source.unlink()
            # A valid cache needs no network or source file at all.
            fetch(source.as_uri(), cached, digest)
            self.assertFalse(cached.with_suffix(".zip.partial").exists())

    def test_failure_preserves_existing_cache_and_removes_partial_input(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source, cached = root / "source.zip", root / "cached.zip"
            cached.write_bytes(b"previous version")
            source.write_bytes(b"corrupt input")
            with self.assertRaisesRegex(RuntimeError, "SHA-256 mismatch"):
                fetch(source.as_uri(), cached, "0" * 64)
            self.assertEqual(cached.read_bytes(), b"previous version")
            self.assertFalse(cached.with_suffix(".zip.partial").exists())

            def interrupted(url, destination):
                Path(destination).write_bytes(b"truncated input")
                raise OSError("connection lost")

            with patch("scripts.pinned_download.urllib.request.urlretrieve", interrupted):
                with self.assertRaisesRegex(OSError, "connection lost"):
                    fetch(source.as_uri(), cached, "0" * 64)
            self.assertEqual(cached.read_bytes(), b"previous version")
            self.assertFalse(cached.with_suffix(".zip.partial").exists())

    def test_large_input_hashing_has_bounded_memory(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "large.zip"
            block = b"x" * (1024 * 1024)
            expected = hashlib.sha256()
            with path.open("wb") as stream:
                for _ in range(32):
                    stream.write(block)
                    expected.update(block)
            tracemalloc.start()
            try:
                self.assertEqual(sha256_file(path), expected.hexdigest())
                _, peak = tracemalloc.get_traced_memory()
            finally:
                tracemalloc.stop()
            self.assertLess(peak, 1024 * 1024)


if __name__ == "__main__":
    unittest.main()
