#!/usr/bin/env python3
"""Build/lint all Android products using validated application version inputs."""
import argparse
import os
import platform
import subprocess
from build_config import ROOT, add_arguments, from_args, record


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    add_arguments(parser)
    args = parser.parse_args()
    config = from_args("android", args)
    record(config)
    env = dict(os.environ, **config.environment())
    mode = config.configuration
    wrapper = "gradlew.bat" if platform.system() == "Windows" else "./gradlew"
    tasks = [f":app-receiver:assembleStandard{mode}", f":app-receiver:assembleLegacy{mode}",
             f":app-sender:assemble{mode}", f":app-receiver:lintStandard{mode}",
             f":app-receiver:lintLegacy{mode}", f":app-sender:lint{mode}", f":control-bridge:test{mode}UnitTest"]
    subprocess.run([wrapper, *tasks, "--stacktrace"], cwd=ROOT / "android", env=env, check=True)
    for flavor in ("legacy", "standard"):
        with (ROOT / "android" / f"{flavor}-dependencies.txt").open("w", encoding="utf-8") as output:
            subprocess.run([wrapper, ":app-receiver:dependencies", "--configuration", f"{flavor}{mode}RuntimeClasspath"],
                           cwd=ROOT / "android", env=env, stdout=output, check=True)
    if "androidx.media3" in (ROOT / "android/legacy-dependencies.txt").read_text(encoding="utf-8"):
        raise RuntimeError("Media3 leaked into Legacy receiver")
    if "media3-common:1.11.1" not in (ROOT / "android/standard-dependencies.txt").read_text(encoding="utf-8"):
        raise RuntimeError("Standard receiver Media3 version mismatch")


if __name__ == "__main__":
    main()
