"""Architecture policy shared by the CLI and regression tests.

Manifest checks cover target, build, test, renamed and workspace dependencies.
Source checks are lexical guardrails, not a Rust/Swift parser.
"""
from pathlib import Path
import re
import tomllib

PRODUCTION = {
    "crates/cast-domain": {"serde"},
    "crates/cast-core": {"cast-domain"},
    "core": {"cast-adapters", "serde_json"},
}
DEVELOPMENT = {"crates/cast-domain": set(), "crates/cast-core": set(), "core": {"tempfile"}}
ANDROID = 'cfg(target_os = "android")'


def dependency_name(alias, spec, workspace):
    if isinstance(spec, dict):
        if spec.get("workspace"):
            if alias not in workspace:
                raise ValueError(f"Unresolved workspace dependency: {alias}")
            spec = workspace[alias]
        if isinstance(spec, dict):
            return spec.get("package", alias)
    return alias


def manifest_errors(directory, manifest, workspace=None):
    workspace = workspace or {}
    errors = []
    tables = [(None, manifest), *manifest.get("target", {}).items()]
    for target, table in tables:
        for section in ("dependencies", "build-dependencies", "dev-dependencies"):
            allowed = set(PRODUCTION[directory]) if section == "dependencies" else (
                set(DEVELOPMENT[directory]) if section == "dev-dependencies" else set()
            )
            if directory == "core" and target == ANDROID and section == "dependencies":
                allowed.add("jni")
            for alias, spec in table.get(section, {}).items():
                try:
                    package = dependency_name(alias, spec, workspace)
                except ValueError as error:
                    errors.append(f"{directory}: {error}")
                    continue
                if package not in allowed:
                    errors.append(f"{directory}: {target or 'all targets'} {section}: forbidden {alias} ({package})")
    return errors


def rust_source_errors(directory):
    errors = []
    io = re.compile(r"\b(?:std\s*::\s*(?:fs|net|process|thread)\b|"
                    r"std\s*::\s*\{[^;]*\b(?:fs|net|process|thread)\b|"
                    r"(?:tokio|async_std|reqwest|windows)\s*::)", re.DOTALL)
    for path in directory.rglob("*.rs"):
        if io.search(path.read_text(encoding="utf-8")):
            errors.append(f"Pure-layer I/O dependency: {path}")
    return errors


def check(root):
    root = Path(root)
    errors = []
    workspace = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8")).get("workspace", {}).get("dependencies", {})
    for directory in PRODUCTION:
        manifest = tomllib.loads((root / directory / "Cargo.toml").read_text(encoding="utf-8"))
        errors.extend(manifest_errors(directory, manifest, workspace))
    for directory in ("crates/cast-domain/src", "crates/cast-core/src"):
        errors.extend(rust_source_errors(root / directory))
    for path in (root / "crates/cast-adapters/src/control").rglob("*.rs"):
        text = path.read_text(encoding="utf-8")
        if re.search(r"\b(?:runtime|Runtime|dlna|live_session|media_http)\b", text):
            errors.append(f"Control transport depends on another adapter/composer: {path}")

    def forbid(path, *tokens):
        text = path.read_text(encoding="utf-8")
        for token in tokens:
            if token in text:
                errors.append(f"Forbidden dependency in {path}: {token}")

    forbid(root / "android/player-legacy/build.gradle.kts", "androidx.media3")
    if "gstreamer" in (root / "windows/CMakeLists.txt").read_text(encoding="utf-8").lower():
        errors.append("Windows must not require the retired GStreamer runtime")
    forbid(root / "windows/src/main.cpp", "gst_")
    for path in (root / "apple").rglob("*.swift"):
        if any(part in {".build", "Dependencies", ".swiftpm"} for part in path.parts):
            continue
        if path.name != "CoreSession.swift":
            forbid(path, "lancast_")
        if "Contracts" in path.parts and "Tests" not in path.parts:
            forbid(path, "import SwiftUI", "import LiveKitWebRTC", "import ReplayKit",
                   "import ScreenCaptureKit", "FileManager", "URLSession")
        if path.parent.name == "Media":
            forbid(path, "import SwiftUI", "CoreSession", "SenderModel", "ReceiverModel")
    return errors
