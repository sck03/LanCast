"""Verified build inputs: bounded hashing and atomic cache replacement."""
import hashlib
from pathlib import Path
import urllib.request


def sha256_file(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def fetch(url, destination, digest):
    destination = Path(destination)
    if destination.is_file() and sha256_file(destination) == digest:
        return
    partial = destination.with_suffix(destination.suffix + ".partial")
    try:
        urllib.request.urlretrieve(url, partial)
        if sha256_file(partial) != digest:
            raise RuntimeError("Dependency SHA-256 mismatch: " + destination.name)
        partial.replace(destination)
    finally:
        partial.unlink(missing_ok=True)
