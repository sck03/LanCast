from pathlib import Path
import tempfile
import unittest
from scripts.architecture import ANDROID, manifest_errors, rust_source_errors


class ArchitectureTests(unittest.TestCase):
    def test_forbidden_dependency_cannot_hide_in_a_target_or_build_table(self):
        for section in ("dependencies", "build-dependencies", "dev-dependencies"):
            for target in (None, 'cfg(windows)'):
                table = {section: {"network": {"package": "tokio", "version": "1"}}}
                manifest = table if target is None else {"target": {target: table}}
                with self.subTest(section=section, target=target):
                    self.assertTrue(manifest_errors("crates/cast-core", manifest))

    def test_workspace_and_package_renaming_resolve_to_actual_dependency(self):
        manifest = {"dependencies": {"serde": {"workspace": True}}}
        workspace = {"serde": {"package": "tokio", "version": "1"}}
        self.assertTrue(manifest_errors("crates/cast-domain", manifest, workspace))
        self.assertTrue(manifest_errors("crates/cast-domain", manifest))
        self.assertEqual(manifest_errors("crates/cast-domain", manifest, {"serde": "1"}), [])
        self.assertEqual(manifest_errors("crates/cast-core", {
            "dependencies": {"domain": {"package": "cast-domain", "path": "../cast-domain"}}
        }), [])

    def test_jni_is_only_an_android_production_exception(self):
        self.assertEqual(manifest_errors("core", {
            "target": {ANDROID: {"dependencies": {"jni": "0.21"}}}
        }), [])
        for manifest in (
            {"dependencies": {"jni": "0.21"}},
            {"target": {"cfg(windows)": {"dependencies": {"jni": "0.21"}}}},
            {"target": {ANDROID: {"build-dependencies": {"jni": "0.21"}}}},
        ):
            self.assertTrue(manifest_errors("core", manifest))

    def test_pure_layer_build_dependencies_are_not_normal_dependencies(self):
        self.assertTrue(manifest_errors("crates/cast-core", {
            "build-dependencies": {"cast-domain": {"path": "../cast-domain"}}
        }))

    def test_nested_pure_source_and_grouped_imports_are_scanned(self):
        with tempfile.TemporaryDirectory() as folder:
            nested = Path(folder) / "nested" / "policy.rs"
            nested.parent.mkdir()
            for text in ("use std::fs;", "use std::{fmt, net::TcpStream};", "use tokio::net;"):
                nested.write_text(text, encoding="utf-8")
                self.assertTrue(rust_source_errors(Path(folder)))
            nested.write_text("use std::collections::HashMap;", encoding="utf-8")
            self.assertEqual(rust_source_errors(Path(folder)), [])
