"""Pure transformations for the all-feature Cargo source inventory (SPDX 2.3).

No subprocesses, network access, or report writes belong in this module.
This graph includes build/dev and platform-conditional dependencies; it is not
an assertion about the contents of a shipped executable.
"""
import hashlib
import json
from pathlib import Path
from urllib.parse import quote


SCOPE = "Cargo source resolution: all workspace features and targets, including build/dev dependencies; not a binary SBOM"


def digest(data):
    return hashlib.sha256(data).hexdigest()


def encode_json(value):
    return json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n"


def build_reports(metadata, lock, *, root, commit, lock_sha256, created, dirty):
    """Return legacy inventory, provenance/graph report, and SPDX document.

    Cargo opaque package IDs are used only for joins. Portable identities use
    name/version/source, or a repository-relative path for local packages.
    Missing lock entries or graph nodes fail instead of producing partial data.
    """
    root = Path(root).resolve()
    locked = {(p["name"], p["version"], p.get("source")): p
              for p in lock["package"]}
    packages = {}
    identities = set()
    for p in metadata["packages"]:
        source = p.get("source")
        if source is None:
            try:
                location = Path(p["manifest_path"]).resolve().parent.relative_to(root).as_posix()
            except ValueError as exc:
                raise ValueError("Local dependency is outside the repository") from exc
            source_identity = "path:" + location
        else:
            location = None
            source_identity = source
        identity = (p["name"], p["version"], source_identity)
        if identity in identities or p["id"] in packages:
            raise ValueError("Duplicate package identity")
        identities.add(identity)
        entry = locked.get((p["name"], p["version"], source))
        if entry is None:
            raise ValueError(f"Package absent from Cargo.lock: {p['name']} {p['version']}")
        checksum = entry.get("checksum")
        if checksum is not None and (len(checksum) != 64 or any(c not in "0123456789abcdef" for c in checksum)):
            raise ValueError("Invalid Cargo archive SHA-256")
        stable_id = "SPDXRef-Package-" + digest(json.dumps(identity).encode())
        packages[p["id"]] = {
            "id": stable_id, "name": p["name"], "version": p["version"],
            "source": source, "path": location, "license": p.get("license"),
            "repository": p.get("repository"), "archiveSha256": checksum,
        }
    nodes = {n["id"]: n for n in metadata["resolve"]["nodes"]}
    if set(nodes) != set(packages):
        raise ValueError("Cargo graph and package inventory do not match")
    edges = []
    for package_id, node in nodes.items():
        for dep in node["deps"]:
            if dep["pkg"] not in packages:
                raise ValueError("Unresolved dependency edge")
            kinds = sorted(({"kind": k.get("kind") or "normal", "target": k.get("target")}
                            for k in dep["dep_kinds"]), key=lambda k: (k["kind"], k["target"] or ""))
            edges.append({"from": packages[package_id]["id"], "to": packages[dep["pkg"]]["id"],
                          "alias": dep["name"], "kinds": kinds})
    edges.sort(key=lambda e: (e["from"], e["to"], e["alias"]))
    ordered = sorted(packages.values(), key=lambda p: p["id"])
    roots = sorted(packages[p]["id"] for p in metadata["workspace_members"])
    inventory = {
        "schema": 1, "scope": SCOPE, "sourceCommit": commit, "dirty": dirty,
        "cargoLockSha256": lock_sha256, "workspaceMembers": roots,
        "packages": ordered, "dependencies": edges,
        "features": {packages[p]["id"]: sorted(n["features"]) for p, n in nodes.items()},
    }
    spdx_packages = []
    for p in ordered:
        package = {
            "SPDXID": p["id"], "name": p["name"], "versionInfo": p["version"],
            "downloadLocation": "NOASSERTION", "filesAnalyzed": False,
            "licenseConcluded": "NOASSERTION", "licenseDeclared": p["license"] or "NOASSERTION",
            "copyrightText": "NOASSERTION", "primaryPackagePurpose": "SOURCE",
        }
        if p["source"] == "registry+https://github.com/rust-lang/crates.io-index":
            package["downloadLocation"] = f"https://crates.io/api/v1/crates/{quote(p['name'], safe='')}/{quote(p['version'], safe='')}/download"
            package["externalRefs"] = [{"referenceCategory": "PACKAGE-MANAGER", "referenceType": "purl",
                                        "referenceLocator": f"pkg:cargo/{quote(p['name'], safe='')}@{quote(p['version'], safe='')}"}]
        if p["archiveSha256"]:
            package["checksums"] = [{"algorithm": "SHA256", "checksumValue": p["archiveSha256"]}]
        if p["repository"]:
            package["homepage"] = p["repository"]
        package["sourceInfo"] = p["source"] or "Repository path: " + p["path"]
        spdx_packages.append(package)
    relationships = [{"spdxElementId": "SPDXRef-DOCUMENT", "relationshipType": "DESCRIBES", "relatedSpdxElement": p} for p in roots]
    relationships += [{"spdxElementId": a, "relationshipType": "DEPENDS_ON", "relatedSpdxElement": b}
                      for a, b in sorted({(e["from"], e["to"]) for e in edges})]
    document = {
        "spdxVersion": "SPDX-2.3", "dataLicense": "CC0-1.0", "SPDXID": "SPDXRef-DOCUMENT",
        "name": "LanCast Rust source dependencies",
        "documentNamespace": "https://github.com/sck03/LanCast/spdx/" + digest(encode_json(inventory).encode()),
        "creationInfo": {"created": created, "creators": ["Tool: LanCast-dependency-report-1"]},
        "comment": SCOPE + f"; source commit {commit}; dirty={dirty}; Cargo.lock SHA256 {lock_sha256}",
        "packages": spdx_packages, "relationships": relationships,
    }
    # Preserve the previous list schema and fields for existing consumers.
    legacy = [{k: p.get(k) for k in ("name", "version", "license", "license_file", "source", "repository")}
              for p in sorted(metadata["packages"], key=lambda p: packages[p["id"]]["id"])]
    return {"rust-dependencies.json": legacy, "rust-dependency-graph.json": inventory,
            "rust-source.spdx.json": document}
