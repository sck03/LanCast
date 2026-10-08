#!/usr/bin/env python3
"""Build selected control-core feature sets once per ABI, then copy to their products."""
from collections import defaultdict
import shutil
import subprocess
from android_ndk import rust_target
from android_products import selected_products
from build_config import ROOT, resolve


def main():
    groups = defaultdict(list)
    for product in selected_products():
        groups[product.features].append(product)
    for abi in resolve("android").android_abis:
        target, env = rust_target(abi, 21)
        subprocess.run(["rustup", "target", "add", target], check=True)
        for features, products in groups.items():
            command = ["cargo", "build", "--release", "--locked", "--target", target,
                       "-p", "cast-ffi", "--no-default-features"]
            if features:
                command += ["--features", ",".join(features)]
            subprocess.run(command, cwd=ROOT, env=env, check=True)
            for product in products:
                output = ROOT / "android" / product.module / "src" / (product.flavor or "main") / "jniLibs" / abi
                output.mkdir(parents=True, exist_ok=True)
                shutil.copy2(ROOT / "target" / target / "release/liblancast_core.so", output)


if __name__ == "__main__":
    main()
