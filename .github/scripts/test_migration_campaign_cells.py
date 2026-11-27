#!/usr/bin/env python3
"""Executed stages map onto plan cells, and nothing is certified."""
import hashlib
import json
from pathlib import Path
import tempfile
import unittest

import migration_campaign_cells as cells
import migration_plan

OLD, NEW = "a" * 40, "b" * 40
IMAGE = "ghcr.io/example-owner/cma-agent-manager@sha256:"


def plan_bytes():
    inventory = {"collections": [{"name": "agents"}, {"name": "auth_api_bearers"}, {"name": "untouched"}]}
    plan = migration_plan.build(inventory, OLD, IMAGE + "1" * 64, NEW, IMAGE + "2" * 64)
    return json.dumps(plan, sort_keys=True).encode()


def evidence(plan, outcome="UNVERIFIED", stages=None, extra=None):
    rows = []
    for stage in stages or cells.STAGES:
        rows.append({"stage": stage, "source": OLD, "image": "private-image", "collections": ["agents", "auth_api_bearers"],
                     "shapes": ["private-shape"], "checks": {"decode": True, "replay": True, "skipped": False},
                     "samples": 2, "outcome": "PASS", **(extra or {})})
    return {"schema": "cma-migration-kind-evidence-v1", "outcome": outcome,
            "plan_sha256": hashlib.sha256(plan).hexdigest(), "fixtures": rows,
            "commands": [{"argv": ["kubectl", "private-argument"]}]}


class CampaignCellTests(unittest.TestCase):
    def derived(self, value, fixture="api-custody"):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "kind-evidence.private.json"
            path.write_text(json.dumps(value))
            return cells.derive(path, fixture)

    def test_derived_record_keeps_only_public_names_and_hashes(self):
        plan = plan_bytes()
        record = self.derived(evidence(plan))
        self.assertEqual(record["stages"]["old-write"],
                         {"outcome": "PASS", "collections": ["agents", "auth_api_bearers"], "checks": ["decode", "replay"]})
        self.assertNotIn("private", json.dumps(record))

    def test_only_retained_cells_are_executed_and_none_is_certified(self):
        plan = plan_bytes()
        report = cells.aggregate(plan, [self.derived(evidence(plan))])
        self.assertEqual(report["certified_cells"], 0)
        self.assertEqual(report["total_cells"], 3 * 3 * 4)
        # 2 collections x 4 readers on the retained fixture; their 2 new-writer
        # fixtures x 4 readers run without an attributable policy.
        self.assertEqual(report["counts"], {"EXECUTED": 8, "EXECUTED_POLICY_UNATTRIBUTED": 16, "NOT_RUN": 12})
        status = {row["id"]: row for row in report["cells"]}
        self.assertEqual(status["current-collection-agents-retained"]["status"], "EXECUTED")
        self.assertEqual(status["current-collection-agents-retained"]["stages"], ["api-custody:new-read-old-write"])
        self.assertEqual(status["historical-collection-agents-enabled"]["status"], "EXECUTED_POLICY_UNATTRIBUTED")
        self.assertEqual(status["recovery-collection-untouched-retained"]["status"], "NOT_RUN")

    def test_blocked_campaign_or_failed_stage_credits_nothing(self):
        plan = plan_bytes()
        blocked = cells.aggregate(plan, [self.derived(evidence(plan, outcome="BLOCKED"))])
        self.assertEqual(blocked["counts"]["NOT_RUN"], blocked["total_cells"])
        failed = evidence(plan, stages=["old-write"], extra={"outcome": "FAIL"})
        report = cells.aggregate(plan, [self.derived(failed)])
        self.assertEqual(report["counts"]["EXECUTED"], 0)

    def test_new_reader_retained_cells_need_the_new_read_stage(self):
        plan = plan_bytes()
        report = cells.aggregate(plan, [self.derived(evidence(plan, stages=["old-write"]))])
        status = {row["id"]: row["status"] for row in report["cells"]}
        self.assertEqual(status["historical-collection-agents-retained"], "EXECUTED")
        self.assertEqual(status["candidate-collection-agents-retained"], "NOT_RUN")

    def test_a_campaign_on_another_plan_is_refused(self):
        plan = plan_bytes()
        other = evidence(plan)
        other["plan_sha256"] = "c" * 64
        with self.assertRaisesRegex(ValueError, "ran a different plan"):
            cells.aggregate(plan, [self.derived(other)])

    def test_explicit_release_probe_cannot_claim_pairwise_coverage(self):
        plan = plan_bytes()
        value = evidence(plan)
        value["release_probe"] = {"writer_policy": "enabled"}
        with self.assertRaisesRegex(ValueError, "per-role execution evidence"):
            self.derived(value)

    def test_unsafe_collection_names_are_refused_at_derive(self):
        plan = plan_bytes()
        value = evidence(plan)
        value["fixtures"][0]["collections"] = ["agents; secret=value"]
        with self.assertRaisesRegex(ValueError, "plain names"):
            self.derived(value)

    def test_markdown_states_that_no_cell_is_certified(self):
        plan = plan_bytes()
        text = cells.markdown(cells.aggregate(plan, [self.derived(evidence(plan))]))
        self.assertIn("Certified cells: 0", text)
        self.assertIn("executed 8", text)


if __name__ == "__main__":
    unittest.main()
