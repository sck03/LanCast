#!/usr/bin/env python3
"""Inspect this repository's Actions without printing or persisting Git credentials."""
import argparse
import json
import pathlib
import subprocess
import urllib.request

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
    parser.add_argument("--failed-logs", action="store_true")
    args = parser.parse_args()
    credential = subprocess.run(["git", "credential", "fill"], input="protocol=https\nhost=github.com\n\n", text=True, capture_output=True, check=True)
    fields = dict(line.split("=", 1) for line in credential.stdout.splitlines() if "=" in line)
    token = fields.get("password", "")
    opener = urllib.request.build_opener(Redirect())

    def get(endpoint, raw=False):
        request = urllib.request.Request(f"https://api.github.com/repos/{REPO}/{endpoint}", headers={"Authorization": f"Bearer {token}", "Accept": "application/vnd.github+json", "X-GitHub-Api-Version": "2022-11-28"})
        with opener.open(request, timeout=30) as response:
            data = response.read()
        return data if raw else json.loads(data)

    run = get(f"actions/runs/{args.run}") if args.run else get("actions/runs?per_page=1")["workflow_runs"][0]
    jobs = get(f"actions/runs/{run['id']}/jobs?per_page=100")["jobs"]
    print(json.dumps({"run": run["id"], "sha": run["head_sha"], "status": run["status"], "conclusion": run["conclusion"], "url": run["html_url"], "jobs": [{"id": j["id"], "name": j["name"], "status": j["status"], "conclusion": j["conclusion"], "step": next((s["name"] for s in j["steps"] if s["status"] == "in_progress" or s["conclusion"] == "failure"), None)} for j in jobs]}, indent=2))
    if args.failed_logs:
        for job in jobs:
            if job["conclusion"] == "failure":
                path = ROOT / ".cache" / f"ci-{job['id']}.log"
                path.parent.mkdir(exist_ok=True)
                data = get(f"actions/jobs/{job['id']}/logs", raw=True).decode("utf-8", errors="replace")
                path.write_text(data.replace(token, "[REDACTED]") if token else data, encoding="utf-8")
                print(str(path))


if __name__ == "__main__":
    main()
