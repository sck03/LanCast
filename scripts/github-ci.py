#!/usr/bin/env python3
"""Inspect this repository's Actions without printing or persisting Git credentials."""
import argparse
import json
import io
import re
import urllib.parse
import zipfile
from github_api import GitHub, ROOT

PRODUCT_WORKFLOWS = ("windows", "android", "macos", "ios", "tvos")
CHECK_WORKFLOWS = ("core-linux", "native-linux", "airplay")
WORKFLOWS = tuple(name + ".yml" for name in (*PRODUCT_WORKFLOWS, *CHECK_WORKFLOWS))


def run_summary(run):
    return {"workflow": run["path"].split("@", 1)[0].rsplit("/", 1)[-1], "run": run["id"],
            "sha": run["head_sha"], "status": run["status"], "conclusion": run["conclusion"], "url": run["html_url"]}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--run", type=int)
    parser.add_argument("--workflow", choices=WORKFLOWS)
    parser.add_argument("--all", action="store_true", help="Summarize all eight workflows for --sha, including missing runs")
    parser.add_argument("--sha", help="Only inspect runs for this full commit SHA")
    parser.add_argument("--failed-logs", action="store_true")
    parser.add_argument("--logs", action="store_true", help="Save logs for all completed jobs, including passing tests")
    parser.add_argument("--download", help="Download one named build artifact into the task cache")
    parser.add_argument("--artifacts", action="store_true", help="List artifact names and sizes for this run")
    parser.add_argument("--dispatch", action="store_true", help="Run one product or check workflow")
    parser.add_argument("--ref", default="main", help="Workflow branch/tag for manual dispatch")
    parser.add_argument("--source-ref", default="", help="Optional application source branch/tag/commit")
    parser.add_argument("--version")
    parser.add_argument("--build-number")
    parser.add_argument("--configuration", choices=["Debug", "Release"])
    from android_products import SELECTION_LABELS
    parser.add_argument("--product", choices=SELECTION_LABELS, help="Android product selection; omitted uses the workflow default")
    args = parser.parse_args()
    if args.product and (not args.dispatch or args.workflow != "android.yml"):
        parser.error("--product applies only to --dispatch --workflow android.yml")

    if args.dispatch:
        from build_config import CONFIGURATION_LABELS, resolve
        if not args.workflow or args.all or args.run:
            parser.error("Select one --workflow for dispatch")
        inputs = {}
        if args.workflow[:-4] in PRODUCT_WORKFLOWS:
            resolve(args.workflow[:-4], version=args.version, build_number=args.build_number, configuration=args.configuration, environ={})
            # Omitted values must resolve in the selected SOURCE checkout, not this local checkout.
            inputs = {key: value for key, value in {"source_ref": args.source_ref, "version": args.version,
                      "build_number": args.build_number, "configuration": CONFIGURATION_LABELS.get(args.configuration)}.items() if value}
            if args.product:
                inputs["product"] = SELECTION_LABELS[args.product]
        elif any((args.source_ref, args.version, args.build_number, args.configuration)):
            parser.error("Check workflows use --ref and do not accept product build inputs")
        GitHub().request(f"actions/workflows/{args.workflow}/dispatches", payload={"ref":args.ref,"inputs":inputs})
        print(json.dumps({"dispatched":args.workflow,"ref":args.ref,"inputs":inputs}))
        return

    client = GitHub()
    get = client.request
    if args.all:
        if not args.sha or any((args.run, args.workflow, args.download, args.artifacts, args.logs, args.failed_logs)):
            parser.error("Use --all --sha <full workflow commit> without per-run options")
        latest = {}
        for run in client.pages("actions/runs?" + urllib.parse.urlencode({"head_sha": args.sha}), "workflow_runs"):
            summary = run_summary(run)
            latest.setdefault(summary["workflow"], summary)
            if set(WORKFLOWS) <= latest.keys():
                break
        print(json.dumps([latest.get(name, {"workflow": name, "status": "missing"}) for name in WORKFLOWS], indent=2))
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
    jobs = list(client.pages(f"actions/runs/{run['id']}/jobs", "jobs"))
    print(json.dumps({**run_summary(run), "jobs": [{"id": j["id"], "name": j["name"], "status": j["status"], "conclusion": j["conclusion"], "step": next((s["name"] for s in j["steps"] if s["status"] == "in_progress" or s["conclusion"] == "failure"), None)} for j in jobs]}, indent=2))
    artifacts = list(client.pages(f"actions/runs/{run['id']}/artifacts", "artifacts")) if args.artifacts or args.download else []
    if args.artifacts:
        print(json.dumps({"artifacts":[{"name":a["name"], "bytes":a["size_in_bytes"], "expired":a["expired"]} for a in artifacts]}, indent=2))
    if args.failed_logs or args.logs:
        for job in jobs:
            if job["conclusion"] == "failure" or (args.logs and job["status"] == "completed"):
                path = ROOT / ".cache" / f"ci-{job['id']}.log"
                path.parent.mkdir(exist_ok=True)
                data = get(f"actions/jobs/{job['id']}/logs", raw=True).decode("utf-8", errors="replace")
                path.write_text(client.redact(data), encoding="utf-8")
                print(str(path))
    if args.download:
        if not re.fullmatch(r"[A-Za-z0-9._-]+", args.download):
            raise SystemExit("Invalid artifact name")
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
