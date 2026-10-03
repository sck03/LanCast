#!/usr/bin/env python3
"""Build/test/package Windows after preparing the pinned native dependencies."""
import argparse
import os
import subprocess
from build_config import ROOT, add_arguments, from_args, record


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    add_arguments(parser)
    config = from_args("windows", parser.parse_args())
    env = dict(os.environ, **config.environment())
    env["RUSTFLAGS"] = (env.get("RUSTFLAGS", "") + " -C target-feature=+crt-static").strip()
    record(config)

    def run(*command):
        subprocess.run(list(command), cwd=ROOT, env=env, check=True)

    run("cargo", "build", "--release", "--locked", "--target", "x86_64-pc-windows-msvc", "-p", "cast-ffi")
    run("cmake", "-S", "windows", "-B", "windows/build", "-G", "Visual Studio 17 2022", "-A", "x64",
        f"-DCMAKE_PREFIX_PATH={ROOT / '.cache/mbedtls-install'}", f"-DLC_APP_VERSION={config.version}", f"-DLC_BUILD_NUMBER={config.build_number}")
    run("cmake", "--build", "windows/build", "--config", config.configuration)
    run("ctest", "--test-dir", "windows/build", "-C", config.configuration, "--output-on-failure")
    run("pwsh", "-NoProfile", "-File", "scripts/package-windows.ps1", "-Configuration", config.configuration)


if __name__ == "__main__":
    main()
