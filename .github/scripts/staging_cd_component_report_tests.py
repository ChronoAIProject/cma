"""A failed component cannot erase a successful peer or manufacture acceptance."""
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from staging_cd_component_report import main, summarize
from staging_cd_compatibility import GROUP
from staging_cd_set import ALREADY_VERIFIED
from staging_cd_targets import TARGETS


def promotion(component, state="controller-verified"):
    reason = {"controller-verified": "none", "withheld": "deployed-compatibility-unqualified"}[state]
    return {"result": "success", "outputs": {"admit": "success", "deploy": "success",
        "admitted": json.dumps([component]), "failures": "{}",
        "component_result": json.dumps({"components": {component: {"state": state, "reason": reason}}})}}


class ComponentReportTests(unittest.TestCase):
    def assertRow(self, rows, columns, reason="none", cause="-"):
        """A row keeps its outcome columns and explains the first unproven one."""
        text = "\n".join(rows) if isinstance(rows, list) else rows
        matches = [line for line in text.splitlines() if line.startswith(columns + " ")]
        self.assertEqual(len(matches), 1, columns)
        explained = matches[0][len(columns) + 1:]
        self.assertTrue(explained.startswith(f"{reason} | {cause} | "), explained)
        next_step = explained[len(f"{reason} | {cause} | "):-2]
        self.assertTrue(next_step == "-" if reason == "none" else len(next_step) > 10, explained)

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
        self.assertRow(rows, "| web | success | admitted | success | controller-verified |")
        self.assertRow(rows, "| am | failure | skipped | skipped | unreported |", "publish-failure", "unknown")

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
                self.assertRow(rows, f"| {c} | reservation_superseded | skipped | skipped | unreported |",
                               "reservation_superseded", "transient")
                self.assertIn(f"{c} publication skipped: reservation_superseded. This run has no verified {c} release.",
                              rows)
                for other in sorted(TARGETS - {c}):
                    self.assertRow(rows, f"| {other} | success | admitted | success | controller-verified |")
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
                self.assertRow(rows, f"| {c} | reservation_superseded | skipped | skipped | unreported |",
                               "reservation_superseded", "transient")
                sibling = sorted(GROUP - {c})[0].upper().replace("-", "_")
                for change in ({"CD_FINALIZED_" + sibling: ""}, {"CD_PUBLISH_" + sibling: "failure"}):
                    self.assertIs(summarize(dict(env, **change))[1], False)
                with self.assertRaises(ValueError):
                    summarize(dict(env, **{"CD_FINALIZED_" + suffix: "success"}))

    def same_source_skip(self, c):
        """The contract run whose claim on c's source skipped because an older run finalized it."""
        suffix = c.upper().replace("-", "_")
        lanes = {"am": "contract", "runtime": "contract", "runtime-omarchy": "contract", "web": "web"}
        # Develop has advanced past this run, which a newer run covering it does not change.
        return dict(self.lanes(lanes), **{"CD_DISPOSITION_" + suffix: "source_already_final",
                                          "CD_FINALIZED_" + suffix: "", "CD_DEVELOP_ADVANCED": "true",
                                          "CD_PROMOTE_CONTRACT": '{"result":"skipped","outputs":{}}'})

    def test_same_source_skip_always_fails_with_its_next_step(self):
        for c in sorted(GROUP):
            with self.subTest(component=c):
                env = self.same_source_skip(c)
                rows, verified = summarize(env)
                # No run is known to deploy this group, at the head or behind it.
                self.assertIs(verified, False)
                self.assertRow(rows, f"| {c} | source_already_final | skipped | skipped | unreported |",
                               "source_already_final", "operator")
                self.assertIn(f"{c}: an older run finalized this source. The next develop push resumes this "
                              "group's final; push to deploy (a re-run allocates nothing once develop moves on).",
                              rows)
                with tempfile.TemporaryDirectory() as tmp:
                    output = Path(tmp, "outputs")
                    with patch.dict(os.environ, dict(env, GITHUB_OUTPUT=str(output)), clear=True):
                        self.assertEqual(main(), 1)
                    self.assertFalse(output.exists())
                # A newer reservation's skip stays handled.
                env["CD_DISPOSITION_" + c.upper().replace("-", "_")] = "reservation_superseded"
                self.assertIsNone(summarize(env)[1])
        with self.assertRaises(ValueError):
            summarize(dict(self.env(), CD_DISPOSITION_AM="source_finalized"))

    def test_locked_admission_can_reuse_a_prior_proof_without_a_deploy_job(self):
        web = {"result": "success", "outputs": {"admit": "success", "admitted": '["web"]',
               "failures": "{}", "admit_disposition": ALREADY_VERIFIED}}
        env = dict(self.env(), CD_PROMOTE_WEB=json.dumps(web))
        rows, passed = summarize(env)
        self.assertTrue(passed)
        self.assertRow(rows, "| web | success | admitted | skipped | already-verified |")
        self.assertIn("web deployment skipped: a previous staging deployment verified this exact release.", rows)
        self.assertFalse(summarize(dict(env, CD_PUBLISH_AM="failure"))[1])
        for inconsistent in ({"deploy": "success"}, {"admitted": "[]"},
                             {"component_result": '{"components":{}}'}, {"admit": "failure"}):
            reported = dict(web, outputs=dict(web["outputs"], **inconsistent))
            with self.subTest(outputs=inconsistent), self.assertRaises(ValueError):
                summarize(dict(env, CD_PROMOTE_WEB=json.dumps(reported)))

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
                self.assertRow(summary.read_text(), "| web | success | admitted | success | unreported |",
                               "controller-unreported", "unknown")
                self.assertRow(summary.read_text(), "| am | success | admitted | success | controller-verified |")

    def test_joint_deployment_requires_a_verified_result_for_each_admitted_peer(self):
        batch = promotion("am")
        batch["outputs"]["admitted"] = '["am","runtime"]'
        env = dict(self.env(), CD_LANES='{"am":"contract","runtime":"contract"}', CD_PUBLISH_RUNTIME="success",
                   CD_PROMOTE_CONTRACT=json.dumps(batch))
        rows, passed = summarize(env)
        self.assertFalse(passed)
        self.assertRow(rows, "| runtime | success | admitted | success | unreported |", "controller-unreported", "unknown")
        self.assertRow(rows, "| am | success | admitted | success | controller-verified |")

    def test_refusal_or_cancellation_is_a_failed_release(self):
        for key, change in (("admit", "failure"), ("deploy", "cancelled"), ("deploy", "skipped")):
            am = promotion("am")
            am["outputs"][key] = change
            self.assertFalse(summarize(dict(self.env(), CD_PROMOTE_AM=json.dumps(am)))[1])
        am = {"result": "success", "outputs": {"admit": "success", "deploy": "skipped", "admitted": "[]",
                                                 "failures": '{"am":"superseded-source"}'}}
        rows, passed = summarize(dict(self.env(), CD_PROMOTE_AM=json.dumps(am)))
        self.assertFalse(passed)
        self.assertRow(rows, "| am | success | superseded-source | skipped | unreported |", "superseded-source", "transient")

    def test_contract_retry_reports_retained_members_and_withheld_outcome(self):
        grouped = promotion("runtime", "withheld")
        grouped["outputs"]["admitted"] = '["am","runtime"]'
        rows, passed = summarize(dict(self.env(), CD_LANES='{"am":"contract"}', CD_PROMOTE_CONTRACT=json.dumps(grouped)))
        self.assertFalse(passed)
        self.assertRow(rows, "| runtime | retained | admitted | success | withheld |",
                       "deployed-compatibility-unqualified", "change")

    def test_a_retry_that_never_reported_fails_its_lane_with_its_own_reason(self):
        env = self.env()
        pending = promotion("am")
        pending["outputs"].update(deploy_disposition="retry-pending", retried="false")
        rows, passed = summarize(dict(env, CD_PROMOTE_AM=json.dumps(pending)))
        self.assertIs(passed, False)
        self.assertRow(rows, "| am | success | admitted | retry-pending | unreported |", "retry-not-run", "unknown")
        self.assertRow(rows, "| web | success | admitted | success | controller-verified |")
        # A retry's outputs never carry the first attempt's hand-over.
        pending["outputs"]["retried"] = "true"
        with self.assertRaises(ValueError):
            summarize(dict(env, CD_PROMOTE_AM=json.dumps(pending)))
        with patch.dict(os.environ, dict(env, CD_PROMOTE_AM=json.dumps(pending)), clear=True):
            self.assertEqual(main(), 1)

    def test_the_automatic_retry_outcome_is_the_lane_outcome(self):
        env = self.env()
        retried = promotion("am")
        retried["outputs"]["retried"] = "true"
        rows, passed = summarize(dict(env, CD_PROMOTE_AM=json.dumps(retried)))
        self.assertIs(passed, True)
        self.assertIn("am: the first deployment was refused for a transient reason before any change; "
                      "the rows below are its automatic re-admission and retry.", rows)
        self.assertRow(rows, "| am | success | admitted | success | controller-verified |")
        # A retry that itself failed keeps the lane failed.
        retried["result"] = "failure"
        retried["outputs"].update(deploy="failure", component_result="")
        rows, passed = summarize(dict(env, CD_PROMOTE_AM=json.dumps(retried)))
        self.assertIs(passed, False)
        self.assertRow(rows, "| am | success | admitted | failure | unreported |", "deploy-failure", "unknown")

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
            first, next_step = path.read_text().splitlines()
            self.assertEqual(first, "Component outcome evidence is missing or invalid.")
            self.assertTrue(next_step.startswith("Next step: "))
            self.assertNotIn("unexpected", path.read_text())

    def test_every_reachable_row_is_explained_by_a_catalogue_entry(self):
        import itertools
        from ci_outcome import catalogue, entry
        from staging_cd_component_admission import REASONS
        from staging_cd_component_report import CODES, diagnosis
        publishes = ("success", "failure", "skipped", "cancelled", "unknown", "retained", "reservation_superseded",
                     "source_already_final", "superseded-peer", "finalize-incomplete")
        admits = ("admitted", "skipped", "unknown", *sorted(REASONS))
        deploys = ("success", "failure", "skipped", "cancelled", "unknown", "retry-pending")
        states = ("controller-verified", "withheld", "unverified", "unreported", "superseded_by_newer_final")
        seen = set()
        for columns in itertools.product(publishes, admits, deploys, states):
            found = diagnosis(*columns)
            if found is None:
                self.assertEqual(columns[1:], ("admitted", "success", "controller-verified"))
                continue
            self.assertIsNot(entry(*found), catalogue()["uncatalogued"], found)
            if found[0] == "component-report":
                seen.add(found[1])
        self.assertEqual(seen | {"allocation-incomplete", "evidence-invalid"}, set(CODES))
