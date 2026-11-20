"""A failed component cannot erase a successful peer or manufacture acceptance."""
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from staging_cd_component_report import main, summarize
from staging_cd_compatibility import GROUP
from staging_cd_targets import TARGETS


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

    def lanes(self, lanes):
        env = dict(self.env(), CD_LANES=json.dumps(lanes))
        for c, lane in lanes.items():
            suffix = c.upper().replace("-", "_")
            env["CD_PUBLISH_" + suffix] = env["CD_FINALIZED_" + suffix] = "success"
            env["CD_PROMOTE_" + lane.upper().replace("-", "_")] = json.dumps(promotion(c))
        return env

    def test_proved_takeover_in_any_lane_is_handled_but_never_a_verified_release(self):
        for c in sorted(TARGETS):
            with self.subTest(component=c):
                suffix = c.upper().replace("-", "_")
                env = self.lanes({key: key for key in TARGETS})
                self.assertTrue(summarize(env)[1])
                env.update({"CD_DISPOSITION_" + suffix: "reservation_superseded", "CD_FINALIZED_" + suffix: "",
                            "CD_PROMOTE_" + suffix: '{"result":"skipped","outputs":{}}'})
                rows, verified = summarize(env)
                self.assertIsNone(verified)
                self.assertIn(f"| {c} | reservation_superseded | skipped | skipped | unreported |", rows)
                self.assertIn(f"{c} publication skipped: reservation_superseded. This run has no verified {c} release.",
                              rows)
                for other in sorted(TARGETS - {c}):
                    self.assertIn(f"| {other} | success | admitted | success | controller-verified |", rows)
                with tempfile.TemporaryDirectory() as tmp:
                    output = Path(tmp, "outputs")
                    with patch.dict(os.environ, dict(env, GITHUB_OUTPUT=str(output)), clear=True):
                        self.assertEqual(main(), 0)
                    self.assertEqual(output.read_text(), "release_outcome=reservation_superseded\n")
                other = "WEB" if c != "web" else "AM"
                for change in ({"CD_PUBLISH_" + other: "failure"}, {"CD_PUBLISH_" + suffix: "failure"},
                               {"CD_INFRASTRUCTURE_RESULT": "failure"}, {"CD_PROMOTE_" + suffix: json.dumps(promotion(c))}):
                    self.assertIs(summarize(dict(env, **change))[1], False)
                for change in ({"CD_FINALIZED_" + suffix: "success"}, {"CD_DISPOSITION_" + suffix: "superseded"}):
                    with self.assertRaises(ValueError):
                        summarize(dict(env, **change))
        unrouted = dict(self.env(), CD_DISPOSITION_EG="reservation_superseded")
        with self.assertRaises(ValueError):
            summarize(unrouted)

    def test_superseded_joint_lane_requires_each_sibling_to_have_finalized(self):
        for c in sorted(GROUP):
            with self.subTest(component=c):
                suffix = c.upper().replace("-", "_")
                env = self.lanes({"am": "contract", "runtime": "contract", "runtime-omarchy": "contract", "web": "web"})
                env.update({"CD_DISPOSITION_" + suffix: "reservation_superseded", "CD_FINALIZED_" + suffix: "",
                            "CD_PROMOTE_CONTRACT": '{"result":"skipped","outputs":{}}'})
                rows, verified = summarize(env)
                self.assertIsNone(verified)
                self.assertIn(f"| {c} | reservation_superseded | skipped | skipped | unreported |", rows)
                sibling = sorted(GROUP - {c})[0].upper().replace("-", "_")
                for change in ({"CD_FINALIZED_" + sibling: ""}, {"CD_PUBLISH_" + sibling: "failure"}):
                    self.assertIs(summarize(dict(env, **change))[1], False)
                with self.assertRaises(ValueError):
                    summarize(dict(env, **{"CD_FINALIZED_" + suffix: "success"}))

    def test_same_source_skip_is_handled_only_when_a_newer_run_will_deploy(self):
        lanes = {"am": "contract", "runtime": "contract", "runtime-omarchy": "contract", "web": "web"}
        for c in sorted(GROUP):
            for advanced, handled in (("true", True), ("false", False), (None, False)):
                with self.subTest(component=c, advanced=advanced):
                    suffix = c.upper().replace("-", "_")
                    env = dict(self.lanes(lanes), **{"CD_DISPOSITION_" + suffix: "source_already_final",
                                                     "CD_FINALIZED_" + suffix: "",
                                                     "CD_PROMOTE_CONTRACT": '{"result":"skipped","outputs":{}}'})
                    if advanced is not None:
                        env["CD_DEVELOP_ADVANCED"] = advanced
                    rows, verified = summarize(env)
                    self.assertIn(f"| {c} | source_already_final | skipped | skipped | unreported |", rows)
                    resume = (f"{c}: an older run finalized this source. The next push resumes this group's "
                              "final; re-run or push to deploy.")
                    # At the develop head no newer run deploys this group, so the skip is a failure.
                    self.assertIs(verified, None if handled else False)
                    self.assertEqual(resume in rows, not handled)
                    # A newer reservation's skip stays handled even at the head.
                    env["CD_DISPOSITION_" + suffix] = "reservation_superseded"
                    self.assertIsNone(summarize(env)[1])
        with self.assertRaises(ValueError):
            summarize(dict(self.env(), CD_DISPOSITION_AM="source_finalized"))

    def test_successful_job_without_a_result_for_admitted_web_fails_the_report(self):
        for result in (None, "", '{"components":{}}'):
            with self.subTest(result=result), tempfile.TemporaryDirectory() as tmp:
                web = promotion("web")
                if result is None:
                    del web["outputs"]["component_result"]
                else:
                    web["outputs"]["component_result"] = result
                summary = Path(tmp, "summary")
                env = dict(self.env(), CD_PROMOTE_WEB=json.dumps(web), GITHUB_STEP_SUMMARY=str(summary))
                with patch.dict(os.environ, env, clear=True):
                    self.assertEqual(main(), 1)
                self.assertIn("| web | success | admitted | success | unreported |", summary.read_text())
                self.assertIn("| am | success | admitted | success | controller-verified |", summary.read_text())

    def test_joint_deployment_requires_a_verified_result_for_each_admitted_peer(self):
        batch = promotion("am")
        batch["outputs"]["admitted"] = '["am","runtime"]'
        env = dict(self.env(), CD_LANES='{"am":"contract","runtime":"contract"}', CD_PUBLISH_RUNTIME="success",
                   CD_PROMOTE_CONTRACT=json.dumps(batch))
        rows, passed = summarize(env)
        self.assertFalse(passed)
        self.assertIn("| runtime | success | admitted | success | unreported |", rows)
        self.assertIn("| am | success | admitted | success | controller-verified |", rows)

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
