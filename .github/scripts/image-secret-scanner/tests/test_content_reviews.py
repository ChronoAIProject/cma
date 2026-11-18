"""A reviewed non-key expression cannot authorize another candidate or component."""

import copy
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from image_secrets.common import ScanError
from image_secrets.content_reviews import COMPONENTS, read_registry, reviewed_content
from image_secrets.gate import assess


class ContentReviewTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.path = Path(directory.name) / "registry.json"
        self.entry = {
            "rule": "gitleaks.image-secret-assignment",
            "scope": "layer",
            "path": "path-sha256:" + "1" * 64,
            "file_sha256": "2" * 64,
            "candidate_sha256": "3" * 64,
            "line": 12,
            "classification": "library-example",
        }
        self.registry = {
            "schema": "image-content-classifications-v1",
            "components": {key: [] for key in COMPONENTS},
        }
        self.registry["components"]["web"] = [self.entry]
        patched = patch("image_secrets.content_reviews.REGISTRY", self.path)
        patched.start()
        self.addCleanup(patched.stop)
        self.save()

    def save(self):
        self.path.write_text(json.dumps(self.registry))

    def finding(self):
        return dict(
            {key: value for key, value in self.entry.items() if key != "classification"}, finding_id="4" * 64
        )

    def test_exact_content_only_and_component_scope(self):
        result = {"policy": {}, "findings": [self.finding()]}
        self.assertEqual(reviewed_content(result, "web"), {"4" * 64: "library-example"})
        self.assertEqual(reviewed_content(result, "eg"), {})
        for key, value in {
            "path": "path-sha256:" + "5" * 64,
            "file_sha256": "6" * 64,
            "candidate_sha256": "7" * 64,
            "line": 13,
            "scope": "config",
        }.items():
            changed = {"policy": {}, "findings": [dict(self.finding(), **{key: value})]}
            self.assertEqual(reviewed_content(changed, "web"), {})

    def test_unknown_duplicate_and_runtime_entries_refuse(self):
        original = copy.deepcopy(self.registry)
        for component, entries in [
            ("web", [self.entry, self.entry]),
            ("runtime", [self.entry]),
            ("web", [dict(self.entry, rule="content.private-key-header")]),
            ("web", [dict(self.entry, line=True)]),
            ("web", [dict(self.entry, file_sha256="*")]),
        ]:
            self.registry = copy.deepcopy(original)
            self.registry["components"][component] = entries
            self.save()
            with self.assertRaises(ScanError):
                read_registry()

    def test_private_key_cannot_be_waived_by_generic_classification(self):
        result = {
            "policy": {},
            "findings": [{"finding_id": "4" * 64, "rule": "content.private-key-header"}],
            "allowlisted": [],
        }
        with patch("image_secrets.gate.read_classifications", return_value={"4" * 64: "public-material"}):
            assess(result, "credential", self.path)
        self.assertEqual(result["counts"]["blocking"], 1)


if __name__ == "__main__":
    unittest.main()
