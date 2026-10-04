import copy
import json
from pathlib import Path
import unittest

from scripts.dependency_inventory import build_reports, encode_json


class DependencyInventoryTests(unittest.TestCase):
    def fixture(self, root):
        source = "registry+https://github.com/rust-lang/crates.io-index"
        packages = [
            {"id": "local-opaque", "name": "app", "version": "1.0.0", "source": None,
             "manifest_path": str(root / "core/Cargo.toml"), "license": "Apache-2.0"},
            {"id": "registry-opaque", "name": "same", "version": "2.0.0", "source": source, "license": "MIT OR Apache-2.0"},
            {"id": "git-opaque", "name": "same", "version": "2.0.0", "source": "git+https://example.org/lib#abc"},
        ]
        metadata = {"packages": packages, "workspace_members": ["local-opaque"], "resolve": {"nodes": [
            {"id": "local-opaque", "features": ["sender"], "deps": [
                {"pkg": "registry-opaque", "name": "renamed", "dep_kinds": [{"kind": None, "target": None}, {"kind": "build", "target": "cfg(windows)"}]},
                {"pkg": "git-opaque", "name": "test_copy", "dep_kinds": [{"kind": "dev", "target": None}]},
            ]},
            {"id": "registry-opaque", "features": [], "deps": []},
            {"id": "git-opaque", "features": [], "deps": []},
        ]}}
        lock = {"package": [{"name": p["name"], "version": p["version"], **({"source": p["source"]} if p["source"] else {})} for p in packages]}
        lock["package"][1]["checksum"] = "a" * 64
        return metadata, lock

    def build(self, metadata, lock, root):
        return build_reports(metadata, lock, root=root, commit="b" * 40,
                             lock_sha256="c" * 64, created="2026-10-04T00:00:00Z", dirty=False)

    def test_graph_preserves_source_identity_alias_kind_target_and_checksum(self):
        root = Path.cwd()
        reports = self.build(*self.fixture(root), root)
        graph = reports["rust-dependency-graph.json"]
        self.assertEqual(len({p["id"] for p in graph["packages"]}), 3)
        edge = next(e for e in graph["dependencies"] if e["alias"] == "renamed")
        self.assertIn({"kind": "build", "target": "cfg(windows)"}, edge["kinds"])
        spdx = reports["rust-source.spdx.json"]
        registry = next(p for p in spdx["packages"] if p["SPDXID"] == edge["to"])
        self.assertEqual(registry["checksums"], [{"algorithm": "SHA256", "checksumValue": "a" * 64}])
        self.assertEqual(registry["externalRefs"][0]["referenceLocator"], "pkg:cargo/same@2.0.0")
        git = next(p for p in spdx["packages"] if p["sourceInfo"].startswith("git+"))
        self.assertNotIn("checksums", git)
        self.assertEqual(git["licenseDeclared"], "NOASSERTION")
        self.assertEqual(len(spdx["relationships"]), 3)
        self.assertTrue(all(not p["filesAnalyzed"] for p in spdx["packages"]))

    def test_checkout_path_and_metadata_order_do_not_change_reports(self):
        root = Path.cwd()
        first = self.build(*self.fixture(root), root)
        other = root / "different-checkout"
        metadata, lock = self.fixture(other)
        metadata["packages"].reverse()
        metadata["resolve"]["nodes"].reverse()
        second = self.build(metadata, lock, other)
        self.assertEqual(encode_json(first), encode_json(second))
        self.assertNotIn(str(root), json.dumps(first))

    def test_invalid_or_partial_inputs_fail(self):
        root = Path.cwd()
        for mutation in ("lock", "nodes", "edge", "checksum", "external-path", "duplicate"):
            metadata, lock = copy.deepcopy(self.fixture(root))
            if mutation == "lock":
                lock["package"].pop()
            elif mutation == "nodes":
                metadata["resolve"]["nodes"].pop()
            elif mutation == "edge":
                metadata["resolve"]["nodes"][0]["deps"][0]["pkg"] = "missing"
            elif mutation == "checksum":
                lock["package"][1]["checksum"] = "invalid"
            elif mutation == "external-path":
                metadata["packages"][0]["manifest_path"] = str(root.parent / "outside/Cargo.toml")
            else:
                metadata["packages"].append(metadata["packages"][0])
            with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                self.build(metadata, lock, root)


if __name__ == "__main__":
    unittest.main()
