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

    def test_blocked_campaign_names_the_failed_command_without_values(self):
        commands = [{"argv": ["/home/runner/work/_temp/kind", "load", "image-archive"], "exit": 0},
                    {"argv": ["kubectl", "--kubeconfig", "/tmp/private/kubeconfig.private", "--context",
                              "kind-cma-matrix-0123456789", "-n", "cma-migration", "wait",
                              "--for=condition=available", "deployment/identity", "--timeout=90s",
                              "MATRIX_SECRET=private-value"], "exit": 1},
                    {"argv": ["kubectl", "get", "pods"], "exit": 0}]
        text, ok, _ = self.render(evidence(outcome="BLOCKED", reason="command failed: kubectl",
                                           commands=commands, boots=[{"stage": "historical-boot"}]))
        self.assertFalse(ok)
        self.assertIn("- Reason: command failed: kubectl", text)
        self.assertIn("- Last failed command: `kubectl cma-migration wait deployment/identity` (exit 1)", text)
        self.assertIn("- Boots: historical-boot", text)
        self.assertNotIn("private", text)
        self.assertNotIn("kubeconfig", text)
        self.assertNotIn("SECRET", text)

    def test_timed_out_command_is_named(self):
        commands = [{"argv": ["kubectl", "rollout", "status", "deployment/api"], "timed_out": True, "exit": None}]
        text, _, _ = self.render(evidence(outcome="BLOCKED", reason="command timed out: kubectl", commands=commands))
        self.assertIn("`kubectl rollout status deployment/api` (timed out)", text)

    def test_blocked_or_uncleaned_campaign_fails(self):
        for value in (evidence(outcome="BLOCKED"), evidence(outcome="CANCELLED"),
                      evidence(cleanup={"cluster_absent": False})):
            with self.subTest(value=value["outcome"]):
                self.assertFalse(self.render(value)[1])


if __name__ == "__main__":
    unittest.main()
