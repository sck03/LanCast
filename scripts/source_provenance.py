"""Describe local build sources and reject mismatched corresponding-source packages."""
import hashlib
import json
from pathlib import Path
import subprocess


def source_state(root):
    root = Path(root)

    def git(*args):
        return subprocess.check_output(["git", *args], cwd=root)

    commit = git("rev-parse", "HEAD").decode("ascii").strip()
    dirty = bool(git("status", "--porcelain", "-z", "--untracked-files=normal"))
    paths = set(git("ls-files", "-z", "--cached", "--others", "--exclude-standard").split(b"\0")) - {b""}
    fingerprint = hashlib.sha256()
    for raw in sorted(paths):
        path = root / raw.decode("utf-8", errors="surrogateescape")
        fingerprint.update(raw + b"\0")
        if path.is_symlink():
            fingerprint.update(b"link\0" + path.readlink().as_posix().encode("utf-8", errors="surrogateescape"))
        elif path.is_file():
            with path.open("rb") as stream:
                fingerprint.update(b"file\0" + hashlib.file_digest(stream, "sha256").digest())
        elif path.exists():
            raise ValueError(f"Unsupported source entry: {path}")
        else:
            fingerprint.update(b"deleted\0")
    return {"source_commit": commit, "source_dirty": dirty, "source_tree_sha256": fingerprint.hexdigest()}


def require_clean_source(root, build_report=None):
    state = source_state(root)
    if state["source_dirty"]:
        raise ValueError("Corresponding-source packaging requires committed sources; commit local changes first")
    if build_report is not None:
        report = json.loads(Path(build_report).read_text(encoding="utf-8-sig"))
        if any(report.get(key) != value for key, value in state.items()):
            raise ValueError("Build report does not match these clean sources; rebuild before packaging source materials")
    return state
