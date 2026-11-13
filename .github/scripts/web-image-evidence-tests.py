#!/usr/bin/env python3
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "web_image_evidence", Path(__file__).with_name("web-image-evidence.py"))
evidence = importlib.util.module_from_spec(spec)
spec.loader.exec_module(evidence)


class EvidenceBudgetTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.source = self.root / "source"
        self.source.mkdir()
        self.destination = self.root / "upload"

    def file(self, name, size):
        path = self.source / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"x" * size)

    def manifest(self):
        return json.loads((self.destination / "artifact-budget.json").read_text())

    def test_under_budget_preserves_files_and_records_the_policy(self):
        self.file("browser-process.json", 50)
        self.file("case/trace.zip", 500)
        self.assertTrue(evidence.stage(self.source, self.destination))
        self.assertEqual((self.destination / "case/trace.zip").read_bytes(), b"x" * 500)
        self.assertEqual(self.manifest()["copied_bytes"], 550)
        self.assertEqual(self.manifest()["omitted_files"], 0)

    def test_limits_keep_diagnostics_first_and_account_for_omissions(self):
        self.file("browser-events.jsonl", 100)
        self.file("case/trace.zip", 500)
        self.file("case/test-failed-1.png", 200)
        self.file("case/checkpoint.png", 300)
        self.file("huge/trace.zip", 1000)
        budget = evidence.MANIFEST_RESERVE + 800
        evidence.stage(self.source, self.destination, limit=budget, file_limit=600)
        result = self.manifest()
        self.assertEqual(result["copied_bytes"], 800)
        self.assertEqual(result["omitted_bytes"], 1300)
        self.assertEqual({p["disposition"] for p in result["files"]},
                         {"retained", "file_limit", "total_limit"})
        self.assertTrue((self.destination / "case/test-failed-1.png").exists())
        self.assertLessEqual(sum(p.stat().st_size for p in self.destination.rglob("*")
                                 if p.is_file()), budget)

    def test_hidden_files_and_links_cannot_expand_the_upload(self):
        self.file(".playwright-artifacts/partial.zip", 100)
        outside = self.root / "outside"
        outside.write_text("not evidence")
        (self.source / "link.json").symlink_to(outside)
        evidence.stage(self.source, self.destination)
        self.assertEqual(self.manifest()["copied_bytes"], 0)
        self.assertEqual(list(self.destination.iterdir()), [self.destination / "artifact-budget.json"])

    def test_missing_source_fails_but_retains_a_manifest(self):
        self.assertFalse(evidence.stage(self.root / "missing", self.destination))
        self.assertTrue(self.manifest()["source_missing"])

    def test_rejects_overlapping_or_existing_destination(self):
        for target in [self.source, self.source / "nested", self.root]:
            with self.assertRaises(ValueError):
                evidence.stage(self.source, target)
        self.destination.mkdir()
        with self.assertRaises(FileExistsError):
            evidence.stage(self.source, self.destination)

    def test_copy_does_not_trust_the_preceding_file_size(self):
        self.file("growing.zip", 1000)
        target = self.root / "bounded.zip"
        self.assertIsNone(evidence.copy_bounded(self.source / "growing.zip", target, 600))
        self.assertFalse(target.exists())

    def test_inventory_limit_counts_omitted_files_without_copying_them(self):
        for name in ["a.json", "b.json", "c.json"]:
            self.file(name, 10)
        with patch.object(evidence, "MAX_FILES", 2):
            evidence.stage(self.source, self.destination)
        self.assertEqual(len(self.manifest()["files"]), 2)
        self.assertEqual(self.manifest()["omitted_files"], 1)
        self.assertEqual(self.manifest()["copied_bytes"], 20)


if __name__ == "__main__":
    unittest.main()
