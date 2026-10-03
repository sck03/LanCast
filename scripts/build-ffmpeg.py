#!/usr/bin/env python3
"""Pinned LGPL-only mux library, no programs/codecs/network. Requires POSIX build tools."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tarfile
import urllib.request

VERSION = "8.0.1"
SHA256 = "05ee0b03119b45c0bdb4df654b96802e909e0a752f72e4fe3794f487229e5a41"
root = Path(__file__).resolve().parents[1]
p = argparse.ArgumentParser()
p.add_argument("--android", choices=["armeabi-v7a", "arm64-v8a"])
p.add_argument("--prefix", type=Path, required=True)
args = p.parse_args()
cache = root / ".cache"
cache.mkdir(exist_ok=True)
archive = cache / f"ffmpeg-{VERSION}.tar.xz"
if not archive.exists():
    urllib.request.urlretrieve(f"https://ffmpeg.org/releases/ffmpeg-{VERSION}.tar.xz", archive)
if hashlib.sha256(archive.read_bytes()).hexdigest() != SHA256:
    raise SystemExit("FFmpeg source SHA256 mismatch")
source = cache / f"ffmpeg-{VERSION}"
if not source.exists():
    with tarfile.open(archive) as tar:
        tar.extractall(cache, filter="data")
build = cache / ("ffmpeg-build-" + (args.android or "host"))
build.mkdir(exist_ok=True)
prefix = args.prefix.resolve()
options = [str(source / "configure"), "--prefix=" + str(prefix), "--disable-everything", "--disable-programs", "--disable-doc", "--disable-network", "--disable-autodetect", "--disable-avdevice", "--disable-avfilter", "--disable-swscale", "--disable-swresample", "--disable-gpl", "--disable-nonfree", "--disable-shared", "--enable-static", "--enable-pic", "--enable-muxer=mpegts", "--disable-x86asm"]
if args.android:
    ndk = Path(os.environ["ANDROID_NDK_HOME"])
    tools = ndk / "toolchains/llvm/prebuilt/linux-x86_64/bin"
    arch, triple = ("arm", "armv7a-linux-androideabi21") if args.android == "armeabi-v7a" else ("aarch64", "aarch64-linux-android21")
    options += ["--enable-cross-compile", "--target-os=android", "--arch=" + arch, "--cc=" + str(tools / (triple + "-clang")), "--cxx=" + str(tools / (triple + "-clang++")), "--ar=" + str(tools / "llvm-ar"), "--ranlib=" + str(tools / "llvm-ranlib"), "--strip=" + str(tools / "llvm-strip")]
subprocess.run(options, cwd=build, check=True)
subprocess.run(["make", "-j" + str(min(os.cpu_count() or 2, 8))], cwd=build, check=True)
subprocess.run(["make", "install"], cwd=build, check=True)
(prefix / "build-manifest.json").write_text(json.dumps({"version": VERSION, "source_sha256": SHA256, "configure": options, "abi": args.android or "host"}, indent=2))
