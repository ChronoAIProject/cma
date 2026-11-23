#!/usr/bin/env python3
"""The public campaign summary carries verdicts and hashes, never evidence."""
import hashlib
import json
from pathlib import Path
import tempfile
import unittest

import migration_campaign_summary as summary


def evidence(**overrides):
    value = {"schema": "cma-migration-kind-evidence-v1", "outcome": "UNVERIFIED",
             "cleanup": {"cluster_absent": True},
             "fixtures": [{"stage": "old-write", "source": "a" * 40, "image": "private-image-ref",
                           "collections": ["agents", "private_collection"], "shapes": ["private-shape"],
                           "checks": {"decode": True, "replay": True}, "samples": 3, "outcome": "PASS"}],
             "catalog_comparisons": [{"to_stage": "new-read-old-write", "outcome": "PRESERVED"}],
             "commands": [{"argv": ["kubectl", "private-argument"]}]}
    value.update(overrides)
    return value


class CampaignSummaryTests(unittest.TestCase):
    def render(self, value):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "kind-evidence.private.json"
            path.write_text(json.dumps(value))
            text, ok = summary.summary(path, "api-custody")
            return text, ok, hashlib.sha256(path.read_bytes()).hexdigest()

    def test_success_publishes_hash_and_verdicts_only(self):
        text, ok, digest = self.render(evidence())
        self.assertTrue(ok)
        self.assertIn(digest, text)
        self.assertIn("| old-write | `aaaaaaaaaaaa` | PASS | 2 | 2/2 |", text)
        self.assertIn("PRESERVED", text)
        self.assertNotIn("private", text)

    def test_blocked_or_uncleaned_campaign_fails(self):
        for value in (evidence(outcome="BLOCKED"), evidence(outcome="CANCELLED"),
                      evidence(cleanup={"cluster_absent": False})):
            with self.subTest(value=value["outcome"]):
                self.assertFalse(self.render(value)[1])


if __name__ == "__main__":
    unittest.main()
