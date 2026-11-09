#!/usr/bin/env python3
"""Preview, remove or quarantine known, ignored outputs without following links."""
import argparse
from dataclasses import asdict, dataclass
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import uuid

ROOT = Path(__file__).resolve().parents[1]
BUILD_PATHS = (
    "target", "airplay-native/target", "windows/build", "windows/build-*",
    "media-native/build", "media-native/build-*", "android/.gradle", "android/.kotlin",
    "android/build", "android/*/build", "android/*/.cxx", "apple/build", "apple/Contracts/.build",
    "scripts/__pycache__", "scripts/tests/__pycache__",
    ".cache/ffmpeg-build-*", ".cache/ffmpeg-local-*", ".cache/media-local-*",
    ".cache/local-joins", ".cache/hardware-joins", ".cache/final-hardware-joins",
    ".cache/media-maintenance",
)
DOWNLOAD_PATHS = (
    ".cache/artifacts", ".cache/ffmpeg-8.*", ".cache/actionlint", ".cache/format-tools",
)


@dataclass(frozen=True)
class Snapshot:
    files: int
    bytes: int
    fingerprint: str
    blockers: tuple[str, ...]


def is_link(info):
    return stat.S_ISLNK(info.st_mode) or bool(
        getattr(info, "st_file_attributes", 0) & getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0)
    )


def checked_path(root, relative):
    """Check lexical and resolved containment, including every existing ancestor."""
    root = root.resolve(strict=True)
    relative = Path(relative)
    if relative.is_absolute() or ".." in relative.parts or not relative.parts:
        raise ValueError(f"Not a workspace child: {relative}")
    path = root
    for part in relative.parts:
        path /= part
        if is_link(path.lstat()):
            raise ValueError(f"Directory/file link is protected: {path}")
    resolved = path.resolve(strict=True)
    if resolved == root or not resolved.is_relative_to(root):
        raise ValueError(f"Outside workspace: {path}")
    return resolved


def snapshot(path):
    """One metadata traversal; links are counted but their targets are never visited."""
    files = size = 0
    digest = hashlib.sha256()
    blockers = []
    pending = [path]
    while pending:
        current = pending.pop()
        info = current.lstat()
        relative = current.relative_to(path).as_posix()
        digest.update(repr((relative, info.st_mode, info.st_size, info.st_mtime_ns)).encode("utf-8"))
        if is_link(info):
            blockers.append(f"link: {relative}")
        elif current.name.casefold() == ".git":
            blockers.append(f"nested repository: {relative}")
        if stat.S_ISDIR(info.st_mode) and not is_link(info):
            with os.scandir(current) as entries:
                pending.extend(sorted((Path(entry.path) for entry in entries), reverse=True))
        else:
            files += 1
            size += info.st_size
            if current.suffix.lower() in {".jks", ".keystore", ".p12", ".pfx"}:
                blockers.append(f"signing material: {relative}")
            if not stat.S_ISREG(info.st_mode) and not is_link(info):
                blockers.append(f"special file: {relative}")
    return Snapshot(files, size, digest.hexdigest(), tuple(blockers))


def git(root, *args, **kwargs):
    return subprocess.run(["git", "-C", str(root), *args], capture_output=True,
                          text=True, encoding="utf-8", check=True, **kwargs).stdout


def tracked_paths(root):
    if Path(git(root, "rev-parse", "--show-toplevel").strip()).resolve() != root:
        raise ValueError("Cleanup must run at the Git workspace root")
    return [root / path for path in git(root, "ls-files", "-z", "--cached").split("\0") if path]


def inspect_candidate(root, relative, tracked):
    path = checked_path(root, relative)
    if any(item == path or item.is_relative_to(path) for item in tracked):
        raise ValueError(f"Tracked source is protected: {relative}")
    ignored = subprocess.run(["git", "-C", str(root), "check-ignore", "-q", "--", relative],
                             capture_output=True)
    if ignored.returncode != 0:
        raise ValueError(f"Not an ignored generated path: {relative}")
    measured = snapshot(path)
    if measured.blockers:
        raise ValueError(f"Protected content in {relative}: {', '.join(measured.blockers[:3])}")
    return measured


def inventory(root=ROOT, include_downloads=False):
    root = root.resolve(strict=True)
    tracked = tracked_paths(root)
    paths = {}
    for category, patterns in (("build", BUILD_PATHS), ("download", DOWNLOAD_PATHS if include_downloads else ())):
        for pattern in patterns:
            for path in root.glob(pattern):
                paths[path.relative_to(root)] = category
    entries = []
    selected = []
    for relative in sorted(paths, key=lambda p: (len(p.parts), str(p))):
        if any(parent == relative or parent in relative.parents for parent in selected):
            continue
        selected.append(relative)
        entry = {"path": relative.as_posix(), "category": paths[relative]}
        try:
            entry.update(asdict(inspect_candidate(root, entry["path"], tracked)))
            entry.pop("blockers")
        except (OSError, ValueError) as error:
            entry["blocked"] = str(error)
        entries.append(entry)
    return entries


def validate_plan(root, entries):
    """Revalidate the entire plan before the first mutation."""
    tracked = tracked_paths(root)
    selected = []
    for entry in entries:
        if "blocked" in entry:
            raise ValueError(entry["blocked"])
        path = checked_path(root, entry["path"])
        if any(path == other or path.is_relative_to(other) or other.is_relative_to(path)
               for other in selected):
            raise ValueError(f"Overlapping cleanup paths: {entry['path']}")
        selected.append(path)
        if inspect_candidate(root, entry["path"], tracked).fingerprint != entry["fingerprint"]:
            raise ValueError(f"Files changed since preview: {entry['path']}")


def revalidate(root, entry):
    path = checked_path(root, entry["path"])
    if inspect_candidate(root, entry["path"], tracked_paths(root)).fingerprint != entry["fingerprint"]:
        raise ValueError(f"Files changed before cleanup: {entry['path']}")
    return path


def clean(root, entries):
    root = root.resolve(strict=True)
    validate_plan(root, entries)
    for entry in entries:
        path = revalidate(root, entry)
        if path.is_dir():
            shutil.rmtree(path)
        else:
            path.unlink()


def quarantine(root, entries):
    """Rename outputs into a unique ignored folder; never copy or delete on failure."""
    root = root.resolve(strict=True)
    validate_plan(root, entries)
    if not entries:
        return None
    parent = root / ".manual-delete"
    # Never archive the destination itself or move tracked content into an ignored folder.
    if any(parent == root / e["path"] or parent.is_relative_to(root / e["path"])
           or (root / e["path"]).is_relative_to(parent) for e in entries):
        raise ValueError("Quarantine destination overlaps cleanup paths")
    parent.mkdir(exist_ok=True)
    parent = checked_path(root, ".manual-delete")
    if any(path.is_relative_to(parent) for path in tracked_paths(root)):
        raise ValueError("Tracked quarantine destination is protected")
    if subprocess.run(["git", "-C", str(root), "check-ignore", "-q", "--", ".manual-delete/"],
                      capture_output=True).returncode != 0:
        raise ValueError("Quarantine destination must be Git-ignored")
    destination = parent / (datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ-") + uuid.uuid4().hex[:8])
    destination.mkdir()
    manifest = {"workspace": str(root), "paths": [{**entry, "moved": False} for entry in entries]}
    manifest_path = destination / "manifest.json"

    def save_manifest():
        manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")

    save_manifest()
    for entry in manifest["paths"]:
        try:
            source = revalidate(root, entry)
            target = destination / entry["path"]
            target.parent.mkdir(parents=True, exist_ok=True)
            # Both paths are within the verified workspace; rename preserves bytes
            # and refuses cross-device moves instead of falling back to copy/delete.
            source.rename(target)
            entry["moved"] = True
        except (OSError, ValueError) as error:
            entry["error"] = str(error)
            save_manifest()
            raise
        save_manifest()
    return destination


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    action = parser.add_mutually_exclusive_group()
    action.add_argument("--clean", action="store_true", help="Remove the listed generated paths; default is preview")
    action.add_argument("--quarantine", action="store_true", help="Move outputs into .manual-delete for manual deletion")
    parser.add_argument("--include-downloads", action="store_true", help="Also select reproducible dependency/artifact downloads")
    parser.add_argument("--json", action="store_true", help="Emit a machine-readable size and cleanup report")
    args = parser.parse_args()
    before = snapshot(ROOT)
    entries = inventory(include_downloads=args.include_downloads)
    if args.clean:
        clean(ROOT, entries)
    destination = quarantine(ROOT, entries) if args.quarantine else None
    after = snapshot(ROOT) if args.clean else before
    report = {"mode": "clean" if args.clean else "quarantine" if args.quarantine else "preview", "workspace": str(ROOT),
              "before_bytes": before.bytes, "after_bytes": after.bytes,
              "reclaimable_bytes": sum(entry.get("bytes", 0) for entry in entries),
              "removed_bytes": before.bytes - after.bytes if args.clean else 0, "paths": entries}
    if args.quarantine:
        report.update(quarantine_directory=str(destination) if destination else None,
                      moved_bytes=sum(entry["bytes"] for entry in entries))
        # Renaming does not reclaim disk space; include the new manifest in the total.
        report["after_bytes"] = snapshot(ROOT).bytes
    if args.json:
        print(json.dumps(report, indent=2))
    else:
        for entry in entries:
            print(f"{entry['path']}: {entry.get('blocked') or str(entry['bytes']) + ' bytes'}")
        print(f"Workspace: {report['after_bytes']:,} bytes; "
              f"{'removed' if args.clean else 'reclaimable'}: {report['removed_bytes' if args.clean else 'reclaimable_bytes']:,} bytes")
        if args.quarantine:
            print(f"Manual deletion folder: {destination}; no disk space freed by moving")


if __name__ == "__main__":
    main()
