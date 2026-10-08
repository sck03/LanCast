"""Repository-scoped GitHub requests; credentials stay in memory and off redirects."""
import json
import os
from pathlib import Path
import subprocess
import urllib.parse
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
REPO = "sck03/LanCast"


class Redirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        redirected = super().redirect_request(req, fp, code, msg, headers, newurl)
        if redirected:
            redirected.remove_header("Authorization")
        return redirected


class GitHub:
    def __init__(self):
        self._token = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN")
        if not self._token:
            credential = subprocess.run(["git", "credential", "fill"],
                input=f"url=https://github.com/{REPO}.git\n\n", cwd=ROOT, text=True,
                capture_output=True, check=True, env=dict(os.environ, GIT_TERMINAL_PROMPT="0"))
            fields = dict(line.split("=", 1) for line in credential.stdout.splitlines() if "=" in line)
            self._token = fields.get("password")
        if not self._token:
            raise RuntimeError("No GitHub credential available for this repository")
        self._opener = urllib.request.build_opener(Redirect())

    def request(self, endpoint, raw=False, payload=None, method=None):
        request = urllib.request.Request(f"https://api.github.com/repos/{REPO}/{endpoint}",
            data=None if payload is None else json.dumps(payload).encode(), method=method,
            headers={"Authorization": f"Bearer {self._token}", "Accept": "application/vnd.github+json",
                     "X-GitHub-Api-Version": "2022-11-28", "Content-Type": "application/json"})
        with self._opener.open(request, timeout=30) as response:
            data = response.read()
        return data if raw else (json.loads(data) if data else None)

    def pages(self, endpoint, key):
        page = 1
        separator = "&" if "?" in endpoint else "?"
        while True:
            values = self.request(f"{endpoint}{separator}per_page=100&page={page}")[key]
            yield from values
            if len(values) < 100:
                return
            page += 1

    def redact(self, text):
        return text.replace(self._token, "[REDACTED]")
