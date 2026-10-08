#!/usr/bin/env python3
"""Validate shared product versions and record effective, reproducible build inputs."""
import argparse
from dataclasses import asdict, dataclass
import json
import os
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[1]
PLATFORMS = ("windows", "android", "macos", "ios", "tvos")
CONFIGURATION_LABELS = {"Release": "日常使用（Release）", "Debug": "开发调试（Debug）"}


@dataclass(frozen=True)
class BuildConfig:
    platform: str
    version: str
    build_number: int
    configuration: str
    android_abis: tuple[str, ...]

    @property
    def label(self):
        return f"{self.version}-{self.build_number}-{self.configuration}"

    def environment(self):
        return {
            "LC_VERSION": self.version,
            "LC_BUILD_NUMBER": str(self.build_number),
            "LC_CONFIGURATION": self.configuration,
            "LC_ANDROID_ABIS": ",".join(self.android_abis),
        }


def resolve(platform, *, version=None, build_number=None, configuration=None, android_abis=None, environ=None, defaults=None):
    if platform not in PLATFORMS:
        raise ValueError("Unknown target platform")
    env = os.environ if environ is None else environ
    defaults = json.loads((ROOT / "build-config.json").read_text(encoding="utf-8")) if defaults is None else defaults
    if defaults.get("schema") != 1:
        raise ValueError("Unsupported build-config schema")
    version = version or env.get("LC_VERSION") or defaults["version"]
    if not re.fullmatch(r"(?:0|[1-9][0-9]{0,3})\.(?:0|[1-9][0-9]{0,3})\.(?:0|[1-9][0-9]{0,3})", version):
        raise ValueError("Version must be numeric major.minor.patch (each 0-9999), e.g. 0.4.0")
    number = str(build_number if build_number not in (None, "") else (env.get("LC_BUILD_NUMBER") or defaults["build_number"]))
    if not re.fullmatch(r"[1-9][0-9]{0,4}", number) or int(number) > 65535:
        raise ValueError("Build number must be 1-65535; increment it for Android upgrades")
    configuration = configuration or env.get("LC_CONFIGURATION") or defaults["configuration"][platform]
    configuration = {label: mode for mode, label in CONFIGURATION_LABELS.items()}.get(configuration, configuration)
    if configuration not in ("Debug", "Release"):
        raise ValueError("Configuration must be Debug or Release")
    abis = android_abis or env.get("LC_ANDROID_ABIS") or ",".join(defaults["android_abis"])
    abis = tuple(abis.split(","))
    if not abis or len(abis) != len(set(abis)) or not set(abis) <= {"armeabi-v7a", "arm64-v8a"}:
        raise ValueError("Android ABIs must be armeabi-v7a, arm64-v8a, or both separated by a comma")
    return BuildConfig(platform, version, int(number), configuration, abis)


def record(config):
    report = asdict(config)
    report["source_commit"] = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    report["artifact_label"] = config.label
    report["signing"] = "development or unsigned; no production signing credentials used"
    path = ROOT / "dist/reports" / f"build-{config.platform}.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    return report


def add_arguments(parser):
    parser.add_argument("--version", help="Numeric major.minor.patch; defaults to LC_VERSION or build-config.json")
    parser.add_argument("--build-number", help="Positive build number (1-65535)")
    parser.add_argument("--configuration", choices=("Debug", "Release"))


def from_args(platform, args):
    return resolve(platform, version=args.version, build_number=args.build_number, configuration=args.configuration)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--platform", choices=PLATFORMS, required=True)
    add_arguments(parser)
    parser.add_argument("--android-abis")
    parser.add_argument("--github", action="store_true", help="Export validated values to Actions environment and step outputs")
    args = parser.parse_args()
    config = resolve(args.platform, version=args.version, build_number=args.build_number,
                     configuration=args.configuration, android_abis=args.android_abis)
    if args.github:
        # Every exported value is validated above; no multi-line workflow commands.
        with open(os.environ["GITHUB_ENV"], "a", encoding="utf-8") as stream:
            stream.writelines(f"{key}={value}\n" for key, value in config.environment().items())
        with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as stream:
            stream.write(f"label={config.label}\nconfiguration={config.configuration}\n")
    print(json.dumps(record(config), indent=2))


if __name__ == "__main__":
    main()
