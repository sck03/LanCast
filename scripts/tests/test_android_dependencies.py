import unittest

from scripts.android_dependencies import verify_dependencies
from scripts.android_products import PRODUCTS


class AndroidDependenciesTests(unittest.TestCase):
    def test_gradle_8_and_9_project_notation_preserves_airplay_isolation(self):
        for project in ("project :airplay-receiver", "project ':airplay-receiver'"):
            report = f"+--- {project}\n\\--- androidx.media3:media3-common:1.11.1\n"
            verify_dependencies(PRODUCTS[3], report, "1.11.1")
            with self.assertRaisesRegex(ValueError, "AirPlay dependency isolation"):
                verify_dependencies(PRODUCTS[1], report, "1.11.1")

    def test_similar_project_name_or_diagnostic_text_is_not_a_dependency(self):
        for report in ("+--- project ':airplay-receiver-old'", "Diagnostic for project ':airplay-receiver'"):
            with self.assertRaisesRegex(ValueError, "AirPlay dependency isolation"):
                verify_dependencies(PRODUCTS[3], report, "1.11.1")

    def test_resolved_media3_version_is_checked_instead_of_requested_version(self):
        verify_dependencies(PRODUCTS[1], "\\--- androidx.media3:media3-common:1.10.0 -> 1.11.1", "1.11.1")
        for report in ("", "\\--- androidx.media3:media3-common:1.11.1 -> 1.11.2",
                       "\\--- androidx.media3:media3-common:1.11.10"):
            with self.assertRaisesRegex(ValueError, "Media3 version mismatch"):
                verify_dependencies(PRODUCTS[1], report, "1.11.1")

    def test_sender_and_legacy_reject_transitive_media3(self):
        for product in (PRODUCTS[0], PRODUCTS[2]):
            verify_dependencies(product, "+--- project ':control-bridge'", "1.11.1")
            with self.assertRaisesRegex(ValueError, "Media3 leaked"):
                verify_dependencies(product, "|    +--- androidx.media3:media3-common:1.11.1", "1.11.1")
