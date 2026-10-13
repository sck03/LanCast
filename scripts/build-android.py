#!/usr/bin/env python3
"""Build, lint and package selected Android products with one effective configuration."""
import argparse
import os
import platform
import subprocess
import sys
from android_artifacts import verify_and_package
from android_dependencies import verify_dependencies
from android_products import SELECTION_LABELS, gradle_tasks, selected_products, selection_key
from android_toolchain import versions
from build_config import ROOT, add_arguments, from_args, record


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    add_arguments(parser)
    parser.add_argument("--product", choices=SELECTION_LABELS)
    parser.add_argument("--native", action="store_true", help="Build selected JNI libraries first (requires ANDROID_NDK_HOME)")
    parser.add_argument("--stacktrace", action="store_true", help="Include full Gradle call stacks when diagnosing a failed build")
    args = parser.parse_args()
    config = from_args("android", args)
    key = selection_key(args.product)
    products = selected_products(key)
    report = record(config)
    report["toolchain"] = versions()
    env = dict(os.environ, **config.environment(), LC_ANDROID_PRODUCT=key)
    mode = config.configuration
    if args.native:
        for component in ("core", "media", "airplay"):
            subprocess.run([sys.executable, str(ROOT / f"scripts/build-android-{component}.py")], cwd=ROOT, env=env, check=True)
    wrapper = "gradlew.bat" if platform.system() == "Windows" else "./gradlew"
    tasks = gradle_tasks(products, mode)
    gradle_options = ["--warning-mode", "all"]
    if args.stacktrace:
        gradle_options.append("--stacktrace")
    subprocess.run([wrapper, *tasks, *gradle_options], cwd=ROOT / "android", env=env, check=True)
    for product in products:
        path = ROOT / "android" / f"{product.key}-dependencies.txt"
        runtime = product.flavor + mode if product.flavor else mode.lower()
        with path.open("w", encoding="utf-8") as output:
            subprocess.run([wrapper, f":{product.module}:dependencies", "--configuration", f"{runtime}RuntimeClasspath"],
                           cwd=ROOT / "android", env=env, stdout=output, check=True)
        verify_dependencies(product, path.read_text(encoding="utf-8"), report["toolchain"]["media3"])
    verify_and_package(ROOT, config, products, report)


if __name__ == "__main__":
    main()
