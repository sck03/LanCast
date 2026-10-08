import contextlib
import importlib.util
import io
from pathlib import Path
import sys
import unittest
from unittest.mock import Mock, patch
import urllib.request

from scripts.github_api import GitHub, Redirect

SCRIPTS = Path(__file__).resolve().parents[1]
with patch.object(sys, "path", [str(SCRIPTS), *sys.path]):
    SPEC = importlib.util.spec_from_file_location("github_ci", SCRIPTS / "github-ci.py")
    CLI = importlib.util.module_from_spec(SPEC)
    SPEC.loader.exec_module(CLI)


class GithubCiTests(unittest.TestCase):
    def dispatch(self, *args):
        with patch.object(sys, "path", [str(SCRIPTS), *sys.path]), patch.object(sys, "argv", ["github-ci.py", "--dispatch", *args]), \
                patch.object(CLI, "GitHub") as client, contextlib.redirect_stdout(io.StringIO()):
            CLI.main()
            return client.return_value.request.call_args

    def test_omitted_inputs_use_remote_source_defaults(self):
        call = self.dispatch("--workflow", "tvos.yml", "--source-ref", "older-version")
        self.assertEqual(call.kwargs["payload"], {"ref": "main", "inputs": {"source_ref": "older-version"}})

    def test_android_product_and_explicit_inputs_are_translated(self):
        call = self.dispatch("--workflow", "android.yml", "--product", "sender", "--version", "1.2.3", "--build-number", "17", "--configuration", "Release")
        inputs = call.kwargs["payload"]["inputs"]
        self.assertEqual(inputs, {"product": "发送端（Android 10+）", "version": "1.2.3", "build_number": "17", "configuration": "日常使用（Release）"})

    def test_protocol_and_native_checks_can_be_dispatched(self):
        for workflow in ("core-linux.yml", "native-linux.yml", "airplay.yml"):
            call = self.dispatch("--workflow", workflow, "--ref", "review-branch")
            self.assertEqual(call.kwargs["payload"], {"ref": "review-branch", "inputs": {}})

    def test_paginated_artifacts_and_redirect_credentials(self):
        client = object.__new__(GitHub)
        client.request = Mock(side_effect=[{"artifacts": list(range(100))}, {"artifacts": [100]}])
        self.assertEqual(list(client.pages("actions/runs/1/artifacts", "artifacts")), list(range(101)))
        self.assertIn("page=2", client.request.call_args.args[0])
        request = urllib.request.Request("https://api.github.com/repos/owner/repo/actions/artifacts/1/zip", headers={"Authorization": "Bearer private"})
        redirected = Redirect().redirect_request(request, None, 302, "Found", {}, "https://example.org/artifact.zip")
        self.assertIsNone(redirected.get_header("Authorization"))


if __name__ == "__main__":
    unittest.main()
