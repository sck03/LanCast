#!/usr/bin/env python3
"""Compile the selected sender ABIs against the source-locked minimal FFmpeg."""
import os
from pathlib import Path
import shutil
import subprocess
import sys
from android_products import selected_products
from build_config import ROOT, resolve


def main():
    if not any(product.role == "sender" for product in selected_products()):
        return
    for abi in resolve("android").android_abis:
        prefix = ROOT / ".cache" / ("ffmpeg-install-" + abi)
        subprocess.run([sys.executable, str(ROOT / "scripts/build-ffmpeg.py"), "--android", abi, "--prefix", str(prefix)], check=True)
        build = ROOT / ".cache" / ("media-build-" + abi)
        subprocess.run(["cmake", "-S", str(ROOT / "media-native"), "-B", str(build), "-G", "Ninja", "-DCMAKE_BUILD_TYPE=Release", "-DANDROID_ABI=" + abi, "-DANDROID_PLATFORM=android-21", "-DANDROID_STL=c++_static", "-DCMAKE_TOOLCHAIN_FILE=" + str(Path(os.environ["ANDROID_NDK_HOME"]) / "build/cmake/android.toolchain.cmake"), "-DFFMPEG_ROOT=" + str(prefix), "-DBUILD_TESTING=OFF"], check=True)
        subprocess.run(["cmake", "--build", str(build)], check=True)
        output = ROOT / "android/app-sender/src/main/jniLibs" / abi
        output.mkdir(parents=True, exist_ok=True)
        shutil.copy2(build / "liblancast_media.so", output)


if __name__ == "__main__":
    main()
