#!/usr/bin/env python3
"""Build the optional GPL AirPlay JNI adapter from reviewed, vendored sources."""
import os
import pathlib
import shutil
import subprocess

ROOT = pathlib.Path(__file__).resolve().parents[1]


def main():
    ndk = pathlib.Path(os.environ["ANDROID_NDK_HOME"])
    host = "windows-x86_64" if os.name == "nt" else "linux-x86_64"
    toolchain = ndk / "toolchains/llvm/prebuilt" / host / "bin"
    targets = [("armv7-linux-androideabi", "armv7a-linux-androideabi23", "armeabi-v7a"),
               ("aarch64-linux-android", "aarch64-linux-android23", "arm64-v8a")]
    for target, clang, abi in targets:
        subprocess.run(["rustup", "target", "add", target], check=True)
        env = os.environ.copy()
        suffix = ".cmd" if os.name == "nt" else ""
        compiler = str(toolchain / (clang + "-clang" + suffix))
        env["CARGO_TARGET_" + target.upper().replace("-", "_") + "_LINKER"] = compiler
        env["CC_" + target.replace("-", "_")] = compiler
        env["AR_" + target.replace("-", "_")] = str(toolchain / ("llvm-ar.exe" if os.name == "nt" else "llvm-ar"))
        env["RUSTFLAGS"] = "-C link-arg=-Wl,-z,max-page-size=16384"
        subprocess.run(["cargo", "build", "--release", "--locked", "--target", target, "-p", "lancast-airplay"],
                       cwd=ROOT / "airplay-native", env=env, check=True)
        output = ROOT / "android/airplay-receiver/src/main/jniLibs" / abi
        output.mkdir(parents=True, exist_ok=True)
        shutil.copy2(ROOT / "airplay-native/target" / target / "release/liblancast_airplay.so", output)


if __name__ == "__main__":
    main()
