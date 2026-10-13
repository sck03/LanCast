"""Read the Android version catalog for CI setup, builds and APK verification."""
import argparse
import json
import os
from pathlib import Path
import re
import tomllib

ROOT = Path(__file__).resolve().parents[1]


def versions(root=ROOT):
    catalog = tomllib.loads((Path(root) / "android/gradle/libs.versions.toml").read_text(encoding="utf-8"))
    values = catalog["versions"]
    for key in ("agp", "kotlin", "buildTools", "media3", "desugar"):
        if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", values[key]):
            raise ValueError(f"Pin a stable Android toolchain version: {key}")
    if not re.fullmatch(r"[1-9][0-9]+", values["compileSdk"]):
        raise ValueError("Invalid compile SDK")
    return values


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--github", action="store_true")
    args = parser.parse_args()
    values = versions()
    if args.github:
        with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as stream:
            stream.write(f"compile_sdk={values['compileSdk']}\nbuild_tools={values['buildTools']}\n")
    print(json.dumps(values, indent=2))


if __name__ == "__main__":
    main()
