#!/usr/bin/env python3
"""Compile only the sender TS JNI bridge against the source-locked minimal FFmpeg."""
import os
from pathlib import Path
import shutil
import subprocess
import sys
root = Path(__file__).resolve().parents[1]
for abi in ("armeabi-v7a", "arm64-v8a"):
    prefix = root / ".cache" / ("ffmpeg-install-" + abi)
    subprocess.run([sys.executable, str(root / "scripts/build-ffmpeg.py"), "--android", abi, "--prefix", str(prefix)], check=True)
    build = root / ".cache" / ("media-build-" + abi)
    subprocess.run(["cmake", "-S", str(root / "media-native"), "-B", str(build), "-G", "Ninja", "-DCMAKE_BUILD_TYPE=Release", "-DANDROID_ABI=" + abi, "-DANDROID_PLATFORM=android-21", "-DANDROID_STL=c++_static", "-DCMAKE_TOOLCHAIN_FILE=" + str(Path(os.environ["ANDROID_NDK_HOME"]) / "build/cmake/android.toolchain.cmake"), "-DFFMPEG_ROOT=" + str(prefix), "-DBUILD_TESTING=OFF"], check=True)
    subprocess.run(["cmake", "--build", str(build)], check=True)
    output = root / "android/app-sender/src/main/jniLibs" / abi
    output.mkdir(parents=True, exist_ok=True)
    shutil.copy2(build / "liblancast_media.so", output)
