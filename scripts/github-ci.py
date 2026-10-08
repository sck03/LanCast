#!/usr/bin/env python3
"""Inspect this repository's Actions without printing or persisting Git credentials."""
import argparse
import json
import io
import pathlib
import re
import subprocess
import urllib.request
import urllib.parse
import zipfile

REPO = "sck03/LanCast"
ROOT = pathlib.Path(__file__).resolve().parents[1]


class Redirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        redirected = super().redirect_request(req, fp, code, msg, headers, newurl)
        if redirected:
            redirected.remove_header("Authorization")
        return redirected


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--run", type=int)
    parser.add_argument("--workflow", help="Workflow filename, for example windows.yml or android.yml")
    parser.add_argument("--sha", help="Only inspect runs for this full commit SHA")
    parser.add_argument("--failed-logs", action="store_true")
    parser.add_argument("--logs", action="store_true", help="Save logs for all completed jobs, including passing tests")
    parser.add_argument("--download", help="Download one named build artifact into the task cache")
    parser.add_argument("--artifacts", action="store_true", help="List artifact names and sizes for this run")
    parser.add_argument("--dispatch", action="store_true", help="Run one independent product workflow")
    parser.add_argument("--ref", default="main", help="Workflow branch/tag for manual dispatch")
    parser.add_argument("--source-ref", default="", help="Optional application source branch/tag/commit")
    parser.add_argument("--version")
    parser.add_argument("--build-number")
    parser.add_argument("--configuration", choices=["Debug", "Release"])
    args = parser.parse_args()
    credential = subprocess.run(["git", "credential", "fill"], input="protocol=https\nhost=github.com\n\n", text=True, capture_output=True, check=True)
    fields = dict(line.split("=", 1) for line in credential.stdout.splitlines() if "=" in line)
    token = fields.get("password", "")
    opener = urllib.request.build_opener(Redirect())

    def get(endpoint, raw=False, payload=None):
        request = urllib.request.Request(f"https://api.github.com/repos/{REPO}/{endpoint}", data=None if payload is None else json.dumps(payload).encode(), headers={"Authorization": f"Bearer {token}", "Accept": "application/vnd.github+json", "X-GitHub-Api-Version": "2022-11-28", "Content-Type":"application/json"})
        with opener.open(request, timeout=30) as response:
            data = response.read()
        return data if raw else (json.loads(data) if data else None)

    if args.dispatch:
        from build_config import CONFIGURATION_LABELS, resolve
        if args.workflow not in [p + ".yml" for p in ("windows", "android", "macos", "ios", "tvos")]:
            raise SystemExit("Select an independent product --workflow (windows/android/macos/ios/tvos.yml)")
        config = resolve(args.workflow[:-4], version=args.version, build_number=args.build_number, configuration=args.configuration, environ={})
        inputs = {"source_ref":args.source_ref, "version":config.version, "build_number":str(config.build_number), "configuration":CONFIGURATION_LABELS[config.configuration]}
        get(f"actions/workflows/{args.workflow}/dispatches", payload={"ref":args.ref,"inputs":inputs})
        print(json.dumps({"dispatched":args.workflow,"ref":args.ref,"inputs":inputs}))
        return

    if args.run:
        run = get(f"actions/runs/{args.run}")
    else:
        endpoint = "actions/runs"
        if args.workflow:
            endpoint = f"actions/workflows/{urllib.parse.quote(args.workflow, safe='')}/runs"
        query = {"per_page": 1}
        if args.sha:
            query["head_sha"] = args.sha
        runs = get(endpoint + "?" + urllib.parse.urlencode(query))["workflow_runs"]
        if not runs:
            raise SystemExit("No matching Actions run yet")
        run = runs[0]
    jobs = get(f"actions/runs/{run['id']}/jobs?per_page=100")["jobs"]
    print(json.dumps({"run": run["id"], "sha": run["head_sha"], "status": run["status"], "conclusion": run["conclusion"], "url": run["html_url"], "jobs": [{"id": j["id"], "name": j["name"], "status": j["status"], "conclusion": j["conclusion"], "step": next((s["name"] for s in j["steps"] if s["status"] == "in_progress" or s["conclusion"] == "failure"), None)} for j in jobs]}, indent=2))
    if args.artifacts:
        artifacts = get(f"actions/runs/{run['id']}/artifacts?per_page=100")["artifacts"]
        print(json.dumps({"artifacts":[{"name":a["name"], "bytes":a["size_in_bytes"], "expired":a["expired"]} for a in artifacts]}, indent=2))
    if args.failed_logs or args.logs:
        for job in jobs:
            if job["conclusion"] == "failure" or (args.logs and job["status"] == "completed"):
                path = ROOT / ".cache" / f"ci-{job['id']}.log"
                path.parent.mkdir(exist_ok=True)
                data = get(f"actions/jobs/{job['id']}/logs", raw=True).decode("utf-8", errors="replace")
                path.write_text(data.replace(token, "[REDACTED]") if token else data, encoding="utf-8")
                print(str(path))
    if args.download:
        if not re.fullmatch(r"[A-Za-z0-9._-]+", args.download):
            raise SystemExit("Invalid artifact name")
        artifacts = get(f"actions/runs/{run['id']}/artifacts?per_page=100")["artifacts"]
        selected = next((a for a in artifacts if a["name"] == args.download and not a["expired"]), None)
        if not selected:
            raise SystemExit("Artifact not available in this run")
        destination = (ROOT / ".cache/artifacts" / str(run["id"]) / args.download).resolve()
        with zipfile.ZipFile(io.BytesIO(get(f"actions/artifacts/{selected['id']}/zip", raw=True))) as archive:
            if len(archive.infolist()) > 10000 or sum(e.file_size for e in archive.infolist()) > 1024**3:
                raise SystemExit("Artifact exceeds extraction limit")
            for entry in archive.infolist():
                if not (destination / entry.filename).resolve().is_relative_to(destination):
                    raise SystemExit("Unsafe artifact path")
            destination.mkdir(parents=True, exist_ok=True)
            archive.extractall(destination)
        print(str(destination))


if __name__ == "__main__":
    main()
