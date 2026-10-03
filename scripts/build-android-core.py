#!/usr/bin/env python3
"""Build JNI libraries with the pinned NDK; no cargo-ndk or system toolchain guessing."""
import os, pathlib, subprocess, shutil, sys
root = pathlib.Path(__file__).resolve().parents[1]
ndk = pathlib.Path(os.environ["ANDROID_NDK_HOME"])
host = "windows-x86_64" if os.name == "nt" else "linux-x86_64"
toolchain = ndk / "toolchains/llvm/prebuilt" / host / "bin"
targets = [("armv7-linux-androideabi","armv7a-linux-androideabi21","armeabi-v7a"), ("aarch64-linux-android","aarch64-linux-android21","arm64-v8a")]
for target, clang, abi in targets:
    subprocess.run(["rustup","target","add",target], check=True)
    env = os.environ.copy()
    suffix = ".cmd" if os.name == "nt" else ""
    compiler = str(toolchain / (clang + "-clang" + suffix))
    env["CARGO_TARGET_" + target.upper().replace("-","_") + "_LINKER"] = compiler
    env["CC_" + target.replace("-","_")] = compiler
    env["AR_" + target.replace("-","_")] = str(toolchain / ("llvm-ar.exe" if os.name == "nt" else "llvm-ar"))
    env["RUSTFLAGS"] = "-C link-arg=-Wl,-z,max-page-size=16384"
    subprocess.run(["cargo","build","--release","--locked","--target",target,"-p","lancast-core"],cwd=root,env=env,check=True)
    output = root / "android/control-bridge/src/main/jniLibs" / abi
    output.mkdir(parents=True, exist_ok=True)
    shutil.copy2(root / "target" / target / "release/liblancast_core.so", output)
