import ast
from fnmatch import fnmatchcase
from pathlib import Path
import re
import unittest


ROOT = Path(__file__).resolve().parents[2]


class WorkflowInputsTests(unittest.TestCase):
    def test_shared_build_and_validation_inputs_trigger_their_platform_workflows(self):
        for workflow, inputs in {
            "windows": ("scripts/windows_artifacts.py", "scripts/artifact_checks.py", "scripts/source_provenance.py"),
            "android": ("scripts/android_artifacts.py", "scripts/source_provenance.py"),
            "macos": ("scripts/artifact_checks.py", "scripts/source_provenance.py"),
            "ios": ("scripts/artifact_checks.py", "scripts/source_provenance.py"),
            "tvos": ("scripts/artifact_checks.py", "scripts/source_provenance.py"),
            "airplay": ("scripts/source_provenance.py",),
        }.items():
            text = (ROOT / ".github/workflows" / f"{workflow}.yml").read_text(encoding="utf-8")
            selections = re.findall(r"(?m)^    paths: (\[.*\])$", text)
            self.assertEqual(len(selections), 2, workflow)
            for selection in selections:
                patterns = ast.literal_eval(selection)
                for source in inputs:
                    with self.subTest(workflow=workflow, source=source):
                        self.assertTrue(any(fnmatchcase(source, pattern) for pattern in patterns))
