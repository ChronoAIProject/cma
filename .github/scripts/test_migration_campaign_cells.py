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


def release_plan():
    plan = json.loads(plan_bytes())
    recovery = migration_plan.identity("c" * 40, IMAGE + "3" * 64)
    plan["readers"][-1]["artifact"] = recovery
    return json.dumps(plan, sort_keys=True).encode()


def release_evidence(plan, policy):
    value = evidence(plan)
    artifacts = {row["role"]: row["artifact"] for row in json.loads(plan)["readers"]}
    controls = []
    for fixture in value["fixtures"]:
        stage = fixture["stage"]
        role = cells.STAGE_ROLES[stage]
        selection = artifacts[role]
        fixture.update(selection)
        fixture["checks"].pop("skipped")
        controls.append({"stage": stage, **selection,
                         "writer_policy": policy if role == "candidate" else "disabled"})
    value["release_probe"] = {"writer_policy": policy, "artifacts": artifacts, "stage_controls": controls}
    value["cleanup"] = {"cluster_absent": True}
    return value


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

    def test_release_credit_binds_each_role_and_the_original_writer_policy(self):
        plan = release_plan()
        record = self.derived(release_evidence(plan, "enabled"))
        self.assertNotIn(IMAGE, json.dumps(record))
        report = cells.aggregate(plan, [record])
        self.assertEqual(report["counts"], {"EXECUTED": 8, "EXECUTED_POLICY_UNATTRIBUTED": 0, "NOT_RUN": 28})
        self.assertEqual(report["certified_cells"], 0)
        status = {row["id"]: row for row in report["cells"]}
        self.assertEqual(status["recovery-collection-agents-enabled"]["stages"],
                         ["api-custody:enabled:old-read-new-write"])
        for cell in ("recovery-collection-agents-disabled", "historical-collection-agents-enabled",
                     "candidate-collection-agents-retained"):
            self.assertEqual(status[cell]["status"], "NOT_RUN")
        second = self.derived(release_evidence(plan, "disabled"))
        combined = cells.aggregate(plan, [record, second])
        self.assertEqual(combined["counts"]["EXECUTED"], 12)

    def test_release_reader_credit_requires_the_matching_writer_collection(self):
        plan = release_plan()
        value = release_evidence(plan, "enabled")
        collections = {
            "old-write": ["agents"],
            "new-read-old-write": ["agents", "auth_api_bearers"],
            "new-write": ["agents"],
            "old-read-new-write": ["agents", "auth_api_bearers"],
        }
        for row in value["fixtures"]:
            row["collections"] = collections[row["stage"]]
        record = self.derived(value)
        report = cells.aggregate(plan, [record])
        self.assertEqual(report["counts"], {"EXECUTED": 4,
                                             "EXECUTED_POLICY_UNATTRIBUTED": 0,
                                             "NOT_RUN": 32})
        status = {row["id"]: row for row in report["cells"]}
        self.assertEqual(status["current-collection-auth-api-bearers-retained"]["status"], "NOT_RUN")
        self.assertEqual(status["candidate-collection-auth-api-bearers-enabled"]["status"], "NOT_RUN")
        self.assertEqual(status["recovery-collection-auth-api-bearers-enabled"]["status"], "NOT_RUN")

    def test_release_refuses_wrong_reader_or_policy_and_incomplete_success(self):
        plan = release_plan()
        for expected, corrupt in (
            ("stage reader differs", lambda value: value["release_probe"]["stage_controls"][-1].update(source=OLD)),
            ("stage policy differs", lambda value: value["release_probe"]["stage_controls"][2].update(writer_policy="disabled")),
            ("fixture reader or stage differs", lambda value: value["fixtures"][-1].update(source=OLD)),
            ("incomplete release campaign", lambda value: value["fixtures"].pop()),
            ("fixture checks did not pass", lambda value: value["fixtures"][0]["checks"].update(decode=False)),
            ("release cleanup missing", lambda value: value["cleanup"].update(cluster_absent=False)),
        ):
            with self.subTest(expected=expected):
                value = release_evidence(plan, "enabled")
                corrupt(value)
                with self.assertRaisesRegex(ValueError, expected):
                    self.derived(value)

    def test_explicit_plan_refuses_pairwise_or_different_artifacts(self):
        plan = release_plan()
        with self.assertRaisesRegex(ValueError, "pairwise coverage requires a pairwise plan"):
            cells.aggregate(plan, [self.derived(evidence(plan))])
        value = release_evidence(plan, "disabled")
        value["release_probe"]["artifacts"]["recovery"]["image"] = IMAGE + "4" * 64
        value["release_probe"]["stage_controls"][-1]["image"] = IMAGE + "4" * 64
        value["fixtures"][-1]["image"] = IMAGE + "4" * 64
        with self.assertRaisesRegex(ValueError, "artifact binding differs from plan"):
            cells.aggregate(plan, [self.derived(value)])

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
