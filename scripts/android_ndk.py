"""One NDK environment for the independent control and optional AirPlay libraries."""
import os
from pathlib import Path
import platform

TARGETS = {
    "armeabi-v7a": ("armv7-linux-androideabi", "armv7a-linux-androideabi"),
    "arm64-v8a": ("aarch64-linux-android", "aarch64-linux-android"),
}


def rust_target(abi, api):
    target, clang = TARGETS[abi]
    host = {"Windows": "windows-x86_64", "Linux": "linux-x86_64", "Darwin": "darwin-x86_64"}[platform.system()]
    toolchain = Path(os.environ["ANDROID_NDK_HOME"]) / "toolchains/llvm/prebuilt" / host / "bin"
    suffix = ".cmd" if os.name == "nt" else ""
    compiler = str(toolchain / f"{clang}{api}-clang{suffix}")
    env = os.environ.copy()
    env["CARGO_TARGET_" + target.upper().replace("-", "_") + "_LINKER"] = compiler
    env["CC_" + target.replace("-", "_")] = compiler
    env["AR_" + target.replace("-", "_")] = str(toolchain / ("llvm-ar.exe" if os.name == "nt" else "llvm-ar"))
    env["RUSTFLAGS"] = "-C link-arg=-Wl,-z,max-page-size=16384"
    return target, env
