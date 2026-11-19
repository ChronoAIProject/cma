"""A failed component cannot erase a successful peer or manufacture acceptance."""
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from staging_cd_component_report import main, summarize


def promotion(component, state="controller-verified"):
    reason = {"controller-verified": "none", "withheld": "deployed-compatibility-unqualified"}[state]
    return {"result": "success", "outputs": {"admit": "success", "deploy": "success",
        "admitted": json.dumps([component]), "failures": "{}",
        "component_result": json.dumps({"components": {component: {"state": state, "reason": reason}}})}}


class ComponentReportTests(unittest.TestCase):
    def env(self):
        return {"CD_PLAN_CHANGED": "true", "CD_ALLOCATION_RESULT": "success",
                "CD_INFRASTRUCTURE_RESULT": "success", "CD_LANES": '{"web":"web","am":"am"}',
                "CD_PUBLISH_WEB": "success", "CD_PUBLISH_AM": "success",
                "CD_PROMOTE_WEB": json.dumps(promotion("web")), "CD_PROMOTE_AM": json.dumps(promotion("am"))}

    def test_mixed_outcomes_remain_separate(self):
        env = self.env()
        self.assertTrue(summarize(env)[1])
        env.update(CD_PUBLISH_AM="failure", CD_PROMOTE_AM='{"result":"skipped","outputs":{}}')
        rows, passed = summarize(env)
        self.assertFalse(passed)
        self.assertIn("| web | success | admitted | success | controller-verified |", rows)
        self.assertIn("| am | failure | skipped | skipped | unreported |", rows)

    def test_refusal_or_cancellation_is_a_failed_release(self):
        for key, change in (("admit", "failure"), ("deploy", "cancelled"), ("deploy", "skipped")):
            am = promotion("am")
            am["outputs"][key] = change
            self.assertFalse(summarize(dict(self.env(), CD_PROMOTE_AM=json.dumps(am)))[1])
        am = {"result": "success", "outputs": {"admit": "success", "deploy": "skipped", "admitted": "[]",
                                                 "failures": '{"am":"superseded-source"}'}}
        rows, passed = summarize(dict(self.env(), CD_PROMOTE_AM=json.dumps(am)))
        self.assertFalse(passed)
        self.assertIn("superseded-source", "\n".join(rows))

    def test_contract_retry_reports_retained_members_and_withheld_outcome(self):
        grouped = promotion("runtime", "withheld")
        grouped["outputs"]["admitted"] = '["am","runtime"]'
        rows, passed = summarize(dict(self.env(), CD_LANES='{"am":"contract"}', CD_PROMOTE_CONTRACT=json.dumps(grouped)))
        self.assertFalse(passed)
        self.assertIn("| runtime | retained | admitted | success | withheld |", rows)

    def test_skipped_or_failed_allocation_and_empty_queue(self):
        self.assertTrue(summarize({"CD_PLAN_CHANGED": "false", "CD_ALLOCATION_RESULT": "skipped"})[1])
        self.assertTrue(summarize(dict(self.env(), CD_LANES="{}"))[1])
        self.assertFalse(summarize(dict(self.env(), CD_ALLOCATION_RESULT="failure"))[1])

    def test_unexpected_component_is_refused_without_echoing_payload(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp, "summary")
            with patch.dict(os.environ, dict(self.env(), CD_PROMOTE_AM=json.dumps(promotion("unexpected")),
                                             GITHUB_STEP_SUMMARY=str(path)), clear=True):
                self.assertEqual(main(), 1)
            self.assertEqual(path.read_text(), "Component outcome evidence is missing or invalid.\n")
