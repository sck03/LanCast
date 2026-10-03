#!/usr/bin/env python3
"""Reproducible Apple dependency preparation, Rust C ABI slices and Xcode builds.

Runs on a Mac with Xcode. No paid SDK, cloud media service or signing secret is used.
Device outputs are unsigned development builds; installation requires user signing.
"""
import argparse
import hashlib
import json
import os
import pathlib
import platform
import plistlib
import shutil
import subprocess
import time
import urllib.request
from build_config import add_arguments, from_args, record

ROOT = pathlib.Path(__file__).resolve().parents[1]
APPLE = ROOT / "apple"
CACHE = ROOT / ".cache/apple"
DEPS = APPLE / "Dependencies"
WEBRTC_VERSION = "150.7871.02"
WEBRTC_SHA = "a523cd141d2aa6c3638d49fea1f72b0aafd28a8818c35fc240e08418da3d2fda"
XCODEGEN_VERSION = "2.44.1"
XCODEGEN_SHA = "a2e905fb68446e9bb4008cdfe2e13e3f176d0cbcca828b71770f8e53fca91b73"
TARGETS = {
    "macos": [("macos", ["aarch64-apple-darwin", "x86_64-apple-darwin"])],
    "ios": [("ios", ["aarch64-apple-ios"]), ("ios-simulator", ["aarch64-apple-ios-sim", "x86_64-apple-ios"])],
    "tvos": [("tvos", ["aarch64-apple-tvos"]), ("tvos-simulator", ["aarch64-apple-tvos-sim"])],
}


def run(*args, cwd=ROOT, env=None):
    print("+", " ".join(map(str, args)), flush=True)
    subprocess.run(list(map(str, args)), cwd=cwd, env=env, check=True)


def fetch(url, destination, digest):
    if not destination.exists() or hashlib.sha256(destination.read_bytes()).hexdigest() != digest:
        partial = destination.with_suffix(".partial")
        urllib.request.urlretrieve(url, partial)
        if hashlib.sha256(partial.read_bytes()).hexdigest() != digest:
            partial.unlink()
            raise RuntimeError("Dependency SHA-256 mismatch: " + destination.name)
        partial.replace(destination)


def main(target):
    parser = argparse.ArgumentParser(description=__doc__)
    add_arguments(parser)
    parser.add_argument("--prepare-only", action="store_true")
    args = parser.parse_args()
    args.platform = target
    product = from_args(target, args)
    if platform.system() != "Darwin":
        raise SystemExit("Apple builds require macOS/Xcode; run the Apple Actions workflow.")
    build_inputs = record(product)
    CACHE.mkdir(parents=True, exist_ok=True); DEPS.mkdir(parents=True, exist_ok=True)
    fetch(f"https://github.com/livekit/webrtc-xcframework/releases/download/{WEBRTC_VERSION}/LiveKitWebRTC.xcframework.zip", CACHE / "webrtc.zip", WEBRTC_SHA)
    fetch(f"https://github.com/yonaskolb/XcodeGen/releases/download/{XCODEGEN_VERSION}/xcodegen.zip", CACHE / "xcodegen.zip", XCODEGEN_SHA)
    # ditto preserves versioned framework symlinks, unlike Python ZipFile.extractall.
    run("ditto", "-x", "-k", CACHE / "webrtc.zip", DEPS)
    run("ditto", "-x", "-k", CACHE / "xcodegen.zip", CACHE)
    generator = CACHE / "xcodegen/bin/xcodegen"; generator.chmod(0o755)
    env = dict(os.environ, MACOSX_DEPLOYMENT_TARGET="13.0", IPHONEOS_DEPLOYMENT_TARGET="16.0", TVOS_DEPLOYMENT_TARGET="17.0")
    libraries = []
    for name, targets in TARGETS[args.platform]:
        slices = []
        for rust_target in targets:
            run("rustup", "target", "add", rust_target)
            features = "legacy" if args.platform == "tvos" else "sender,legacy"
            run("cargo", "build", "--release", "--locked", "-p", "cast-ffi", "--no-default-features", "--features", features, "--target", rust_target, env=env)
            slices.append(ROOT / "target" / rust_target / "release/liblancast_core.a")
        output = CACHE / name / "liblancast_core.a"; output.parent.mkdir(parents=True, exist_ok=True)
        if len(slices) > 1: run("lipo", "-create", *slices, "-output", output)
        else: shutil.copy2(slices[0], output)
        libraries += ["-library", str(output), "-headers", str(ROOT / "core/include")]
    framework = DEPS / "LanCastCore.xcframework"
    if framework.exists():
        assert framework.resolve().is_relative_to(DEPS.resolve())
        shutil.rmtree(framework)
    run("xcodebuild", "-create-xcframework", *libraries, "-output", framework)
    run(generator, "generate", "--spec", "project.yml", cwd=APPLE)
    if args.prepare_only: return
    output = ROOT / "dist/apple" / args.platform; output.mkdir(parents=True, exist_ok=True)
    shutil.copy2(DEPS / "LiveKitWebRTC.xcframework/LICENSE", output / "LiveKitWebRTC-LICENSE.txt")
    shutil.copy2(ROOT / "THIRD_PARTY.md", output / "THIRD_PARTY.md")
    shutil.copy2(ROOT / "LICENSE", output / "LanCast-LICENSE.txt")
    scheme = {"macos": "LanCastMac", "ios": "LanCastIOS", "tvos": "LanCastTV"}[args.platform]
    version_settings = [f"MARKETING_VERSION={product.version}", f"CURRENT_PROJECT_VERSION={product.build_number}"]
    common = ["xcodebuild", "-project", str(APPLE / "LanCast.xcodeproj"), "-scheme", scheme, "-configuration", "Debug" if target == "macos" else product.configuration, "-derivedDataPath", str(CACHE / "DerivedData"), *version_settings]
    products = CACHE / "DerivedData/Build/Products"
    if args.platform == "macos":
        run("swift", "test", "--package-path", APPLE / "Contracts")
        run(*common, "test", "-destination", "platform=macOS", "-parallel-testing-enabled", "NO",
            "-test-timeouts-enabled", "YES", "-maximum-test-execution-time-allowance", "90",
            "-resultBundlePath", output / f"tests-{time.time_ns()}.xcresult", "CODE_SIGNING_ALLOWED=YES", "CODE_SIGN_IDENTITY=-", "CODE_SIGN_ENTITLEMENTS=", "ENABLE_HARDENED_RUNTIME=NO")
        # Test hosts contain XCTest bundles and only the runner's architecture. Ship a
        # separately built universal app so Intel Macs receive a compiled executable too.
        universal = CACHE / "MacDelivery"
        run("xcodebuild", "-project", APPLE / "LanCast.xcodeproj", "-scheme", scheme,
            "-configuration", product.configuration, "-derivedDataPath", universal, "build", *version_settings,
            "-destination", "generic/platform=macOS", "ARCHS=arm64 x86_64", "ONLY_ACTIVE_ARCH=NO", "CODE_SIGNING_ALLOWED=NO")
        products = universal / "Build/Products"
    else:
        dest = "iOS" if args.platform == "ios" else "tvOS"
        run(*common, "build", "-destination", f"generic/platform={dest}", "CODE_SIGNING_ALLOWED=NO")
        run(*common, "build", "-destination", f"generic/platform={dest} Simulator", "ARCHS=arm64", "CODE_SIGNING_ALLOWED=NO")
    binaries = []
    for app in products.glob(f"*/{scheme}.app"):
        if not app.parent.name.startswith(product.configuration):
            continue
        info_path = app / ("Contents/Info.plist" if target == "macos" else "Info.plist")
        info = plistlib.loads(info_path.read_bytes())
        if info.get("CFBundleShortVersionString") != product.version or str(info.get("CFBundleVersion")) != str(product.build_number):
            raise RuntimeError("Product version/build number not applied to " + str(app))
        executable = app / (f"Contents/MacOS/{scheme}" if args.platform == "macos" else scheme)
        architectures = subprocess.check_output(["lipo", "-archs", executable], text=True).strip().split()
        if args.platform == "macos" and set(architectures) != {"arm64", "x86_64"}:
            raise RuntimeError("Mac app is missing a required architecture")
        binaries.append({"product": app.parent.name, "architectures": architectures})
        run("ditto", "-c", "-k", "--keepParent", app, output / f"{scheme}-{app.parent.name}.zip")
    if len(binaries) != (1 if target == "macos" else 2):
        raise RuntimeError("Missing or unexpected Apple build products")
    report = {
        "commit": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
        "platform": args.platform, "webrtcVersion": WEBRTC_VERSION, "webrtcSha256": WEBRTC_SHA,
        "buildConfig": build_inputs,
        "xcodegenVersion": XCODEGEN_VERSION, "xcodegenSha256": XCODEGEN_SHA,
        "xcode": subprocess.check_output(["xcodebuild", "-version"], text=True).strip(),
        "rust": subprocess.check_output(["rustc", "--version"], text=True).strip(),
        "targets": TARGETS[args.platform], "deviceSigning": "unsigned; user signing required",
        "binaries": binaries,
        "scope": "Builds and automated tests only; no physical device or performance acceptance",
        "artifacts": [{"file": p.name, "bytes": p.stat().st_size, "sha256": hashlib.sha256(p.read_bytes()).hexdigest()} for p in output.glob("*.zip")],
    }
    (output / "build-report.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")


