#!/usr/bin/env python3
"""Build the optional GPL AirPlay JNI adapter from reviewed, vendored sources."""
import shutil
import subprocess
from android_ndk import rust_target
from android_products import selected_products
from build_config import ROOT, resolve


def main():
    if not any(product.airplay for product in selected_products()):
        return
    for abi in resolve("android").android_abis:
        target, env = rust_target(abi, 23)
        subprocess.run(["rustup", "target", "add", target], check=True)
        subprocess.run(["cargo", "build", "--release", "--locked", "--target", target, "-p", "lancast-airplay"],
                       cwd=ROOT / "airplay-native", env=env, check=True)
        output = ROOT / "android/airplay-receiver/src/main/jniLibs" / abi
        output.mkdir(parents=True, exist_ok=True)
        shutil.copy2(ROOT / "airplay-native/target" / target / "release/liblancast_airplay.so", output)


if __name__ == "__main__":
    main()
