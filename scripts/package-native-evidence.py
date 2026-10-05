#!/usr/bin/env python3
"""Package actual source inputs, library notices, hashes and LGPL relinking sources."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tarfile
import urllib.request
import zipfile

root = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser()
parser.add_argument("platform", choices=["windows", "android"])
args = parser.parse_args()
destination = root / "dist" / "native-evidence" / args.platform
destination.mkdir(parents=True, exist_ok=True)
commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip()
source = destination / "lancast-source.tar.gz"
subprocess.run(["git", "archive", "--format=tar.gz", "-o", str(source), "HEAD"], cwd=root, check=True)
ffmpeg = root / ".cache/ffmpeg-8.0.1.tar.xz"
if not ffmpeg.is_file():
    ffmpeg.parent.mkdir(parents=True, exist_ok=True)
    urllib.request.urlretrieve("https://ffmpeg.org/releases/ffmpeg-8.0.1.tar.xz", ffmpeg)
assert hashlib.sha256(ffmpeg.read_bytes()).hexdigest() == "05ee0b03119b45c0bdb4df654b96802e909e0a752f72e4fe3794f487229e5a41"
with zipfile.ZipFile(destination / "ffmpeg-relink.zip", "w", zipfile.ZIP_DEFLATED) as archive:
    archive.write(ffmpeg, ffmpeg.name)
    for name in ("COPYING.LGPLv2.1", "COPYING.LGPLv3", "LICENSE.md"):
        with tarfile.open(ffmpeg) as upstream:
            archive.writestr("licenses/" + name, upstream.extractfile("ffmpeg-8.0.1/" + name).read())
    for pattern in (".cache/ffmpeg-*/build-manifest.json", ".cache/media-build-*/CMakeFiles/**/*.o", ".cache/ts-windows/CMakeFiles/**/*.obj"):
        for path in root.glob(pattern):
            archive.write(path, str(path.relative_to(root)).replace("\\", "/"))
components = []
if args.platform == "windows":
    directories = [root / "windows/build/_deps/datachannel-src", root / "windows/build/_deps/opus-src", root / "windows/build/_deps/json-src", root / ".cache/mbedtls"]
    directories += [root / "windows/build/_deps/datachannel-src/deps" / name for name in ("libjuice", "libsrtp", "usrsctp", "plog")]
    for directory in directories:
        assert directory.is_dir(), f"Missing dependency source: {directory}"
        revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=directory, text=True).strip()
        output = destination / (directory.name + "-source.tar.gz")
        subprocess.run(["git", "archive", "--format=tar.gz", "-o", str(output), "HEAD"], cwd=directory, check=True)
        changes = subprocess.check_output(["git", "diff", "--binary", "HEAD"], cwd=directory)
        if changes:
            (destination / (directory.name + "-build.patch")).write_bytes(changes)
        components.append({"name": directory.name, "revision": revision, "sourceArchive": output.name})
    # Preserve recursive Mbed TLS submodules too (for example the fixed test framework).
    output = subprocess.check_output(["git", "submodule", "status", "--recursive"], cwd=root / ".cache/mbedtls", text=True)
    for line in output.splitlines():
        revision, relative, *_ = line.strip().split()
        directory = root / ".cache/mbedtls" / relative
        archive = destination / ("mbedtls-" + relative.replace("/", "-") + "-source.tar.gz")
        subprocess.run(["git", "archive", "--format=tar.gz", "-o", str(archive), "HEAD"], cwd=directory, check=True)
        components.append({"name": "mbedtls/" + relative, "revision": revision, "sourceArchive": archive.name})

instructions = """Native source and relinking materials

The FFmpeg archive is the exact LGPL source used by this build. The adjacent LanCast source archive
contains the entire work using that library, including C/C++ JNI wrappers and build scripts.
No FFmpeg source modifications are applied. To modify and relink:
1. Extract lancast-source.tar.gz into an empty directory. Open ffmpeg-relink.zip and
   extract its FFmpeg source archive into .cache/.
2. Modify FFmpeg, update the local build script's source checksum to match your archive,
   and run scripts/build-ffmpeg.py with the same ABI/toolchain in the manifest.
3. Build media-native with CMake and the new FFMPEG_ROOT. See the included CI workflow.
4. Windows: replace lancast_ts.dll next to LanCast.exe. Android: rebuild the sender APK
   with the replacement JNI .so and sign/install it with your own key; uninstall the
   differently signed build first. LanCast imposes no library signature restriction.
The original corresponding wrapper objects are included when available, in addition to
the complete source. Library licenses are inside the archives. Other fixed native sources
and their own notices are separate archives. Application signing credentials are not included.
This package is build/source evidence, not a claim of hardware or television certification.
"""
(destination / "RELINK.txt").write_text(instructions, encoding="utf-8")
artifacts = []
for path in sorted(destination.iterdir()):
    if path.is_file() and path.name != "manifest.json":
        artifacts.append({"file": path.name, "bytes": path.stat().st_size, "sha256": hashlib.sha256(path.read_bytes()).hexdigest()})
(destination / "manifest.json").write_text(json.dumps({"schemaVersion": 1, "commit": commit, "platform": args.platform, "components": components, "artifacts": artifacts}, indent=2), encoding="utf-8")
print(f"Packaged native source and relinking evidence: {destination}")
