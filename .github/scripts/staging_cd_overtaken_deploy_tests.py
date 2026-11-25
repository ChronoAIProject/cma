"""A deployment overtaken by newer finals deploys nothing and is handled, never hidden."""
import contextlib
import fnmatch
import io
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

import staging_cd_invoke as invoke
import staging_cd_invoke_tests as invoke_fixtures
import staging_cd_resolve as resolver
from staging_cd_clock import timestamp
from staging_cd_common import Refusal
from staging_cd_component_admission import automatic_admission, overtaken_admission
from staging_cd_component_report import summarize
from staging_cd_component_report_tests import promotion
from staging_cd_deploy_result_tests import fixture_result
from staging_cd_history import DevelopHistory
from staging_cd_receipts import canonical
from staging_cd_receipts_tests import fixture_receipt, fixture_runtime_receipt, fixture_tag
from staging_cd_resolve import ObsoleteSource
from staging_cd_set import OVERTAKEN, OvertakenByNewerFinal, release_set, verify_set
from staging_cd_watermark import SUPERSEDED, SupersededVersions, later_final_sources, require_current_versions

OLDER, NEWER = "a" * 40, "b" * 40


def receipt(component, source=OLDER, version="0.10.1"):
    previous = f"0.10.{int(version.rsplit('.', 1)[1]) - 1}"
    make = fixture_runtime_receipt if component.startswith("runtime") else fixture_receipt
    return make(component=component, source=source, version=version, previous=previous)


class OvertakenDeploymentTests(unittest.TestCase):
    def fixture(self, keys, *, overtaken, listed=None):
        """An admitted older set; `overtaken` members have a final at a newer develop source."""
        receipts = {key: receipt(key, version="0.10.2" if key == "runtime" else "0.10.1") for key in keys}
        newer = {key: receipt(key, NEWER, "0.10.3" if key == "runtime" else "0.10.2") for key in overtaken}
        listed = newer if listed is None else listed
        api, registry = Mock(), Mock()
        api.history.return_value = DevelopHistory(NEWER, [NEWER, OLDER])
        api.tags.side_effect = lambda key: [fixture_tag(r) for r in (receipts[key], listed.get(key)) if r]
        refs = ["refs/tags/" + r["tag"] for r in (*receipts.values(), *newer.values())]

        def advertise(args, **_kwargs):
            lines = ["1" * 40 + "\t" + ref for ref in refs if any(fnmatch.fnmatchcase(ref, p) for p in args)]
            return subprocess.CompletedProcess(args, 0, "\n".join(lines), "")

        api.require_current_versions.side_effect = lambda selected: require_current_versions(
            selected, runner=advertise)
        registry.exists.return_value = True
        registry.manifest.return_value = ("sha256:" + "d" * 64, {})
        return release_set(receipts), api, registry

    def matrix(self, value):
        return {"include": [{"component": key, "tasks": canonical([
            {"component": key, "mode": "resume", "receipt": item}])} for key, item in value["receipts"].items()]}

    def admit(self, keys, **case):
        value, api, registry = self.fixture(keys, **case)
        matrix = self.matrix(value)
        policy = Mock()
        policy.transition.return_value = False
        policy.signature.return_value = ("contract", "bootstrap")
        admitted, failures = automatic_admission(matrix, api, registry, policy=policy)
        return admitted, failures, overtaken_admission(matrix, admitted, failures, api)

    def test_a_wholly_overtaken_independent_admission_is_handled(self):
        for keys in (["web"], ["eg"], ["eg", "web"]):
            with self.subTest(keys=keys):
                admitted, failures, overtaken = self.admit(keys, overtaken=keys)
                self.assertIsNone(admitted)
                self.assertEqual(failures, {key: "superseded-source" for key in keys})
                self.assertEqual(overtaken, tuple(sorted(keys)))

    def test_partial_mixed_group_or_unproved_admissions_behave_as_today(self):
        admitted, failures, overtaken = self.admit(["eg", "web"], overtaken=["web"])
        self.assertEqual((set(admitted["receipts"]), failures, overtaken), ({"eg"}, {"web": "superseded-source"}, ()))
        for name, case in {"watermark-only": dict(keys=["web"], overtaken=["web"], listed={}),
                           "group-member": dict(keys=["am"], overtaken=["am"]),
                           "mixed-with-group": dict(keys=["am", "web"], overtaken=["am", "web"])}.items():
            with self.subTest(case=name):
                admitted, failures, overtaken = self.admit(**case)
                self.assertIsNone(admitted)
                self.assertEqual(set(failures.values()), {"superseded-source"})
                self.assertEqual(overtaken, ())
        value, api, _ = self.fixture(["eg", "web"], overtaken=["web"])
        # Another reason beside the overtaken member keeps the admission red.
        failures = {"web": "superseded-source", "eg": "publication-unqualified"}
        self.assertEqual(overtaken_admission(self.matrix(value), None, failures, api), ())
        marked = self.matrix(value)
        for entry in marked["include"]:
            entry["contract_transition"] = OLDER
        self.assertEqual(overtaken_admission(marked, None, {"web": "superseded-source"}, api), ())

    def test_admission_step_reports_the_handled_disposition(self):
        value, api, registry = self.fixture(["web"], overtaken=["web"])
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            event = {"ref": "refs/heads/develop", "repository": {"full_name": "ChronoAIProject/cma"},
                     "after": NEWER, "deleted": False}
            (root / "event").write_text(json.dumps(event))
            env = {"GITHUB_REPOSITORY": "ChronoAIProject/cma", "GITHUB_REF": "refs/heads/develop",
                   "GITHUB_EVENT_NAME": "push", "GITHUB_SHA": NEWER, "GITHUB_EVENT_PATH": str(root / "event"),
                   "GITHUB_OUTPUT": str(root / "output"), "GITHUB_STEP_SUMMARY": str(root / "summary"),
                   "GITHUB_RUN_ID": "123", "GITHUB_RUN_ATTEMPT": "1", "RUNNER_TEMP": str(root),
                   "GH_TOKEN": "fixture", "CD_RESOLVE_MODE": "automatic", "CD_PROMOTION_LANE": "web",
                   "CD_PUBLICATION_MATRIX": canonical(self.matrix(value))}
            log = io.StringIO()
            with patch.object(resolver, "GitHub", return_value=api), \
                    patch.object(resolver, "Registry", return_value=registry), \
                    patch.dict(os.environ, env, clear=True), contextlib.redirect_stdout(log):
                self.assertEqual(resolver.main(), 0)
            outputs = dict(line.split("=", 1) for line in (root / "output").read_text().splitlines())
            self.assertEqual((outputs["changed"], outputs["disposition"]), ("false", OVERTAKEN))
            self.assertEqual(json.loads(outputs["failures"]), {"web": "superseded-source"})
            self.assertIn(f"SKIP admission reason={OVERTAKEN} components=web", log.getvalue())
            self.assertIn(f"Admission skipped: {OVERTAKEN} for web.", (root / "summary").read_text())

    def test_every_independent_member_overtaken_by_a_later_final_deploys_nothing(self):
        for keys in (["web"], ["eg"], ["eg", "web"]):
            with self.subTest(keys=keys):
                value, api, registry = self.fixture(keys, overtaken=keys)
                with self.assertRaises(OvertakenByNewerFinal) as error:
                    verify_set(value, api, registry, automatic=True)
                self.assertEqual(error.exception.components, tuple(sorted(keys)))

    def test_a_partly_overtaken_set_still_deploys_its_current_members(self):
        value, api, registry = self.fixture(["eg", "web"], overtaken=["web"])
        current = verify_set(value, api, registry, automatic=True)
        self.assertEqual(set(current["receipts"]), {"eg"})

    def test_any_other_refusal_or_unproved_takeover_still_fails(self):
        cases = {
            # A newer final only the watermark sees is not a proved later-source takeover.
            "watermark-only": dict(keys=["web"], overtaken=["web"], listed={}),
            # A contract member's newer final may sit in a lane that never promotes it.
            "group-member": dict(keys=["am"], overtaken=["am"]),
            "mixed-with-group": dict(keys=["am", "web"], overtaken=["am", "web"]),
        }
        for name, case in cases.items():
            with self.subTest(case=name):
                value, api, registry = self.fixture(**case)
                with self.assertRaisesRegex(Refusal, SUPERSEDED) as error:
                    verify_set(value, api, registry, automatic=True)
                self.assertNotIsInstance(error.exception, OvertakenByNewerFinal)
        value, api, registry = self.fixture(["web"], overtaken=["web"])
        api.require_current_versions.side_effect = Refusal("Component version inventory is unavailable")
        with self.assertRaisesRegex(Refusal, "unavailable") as error:
            verify_set(value, api, registry, automatic=True)
        self.assertNotIsInstance(error.exception, OvertakenByNewerFinal)

    def test_a_contract_transition_keeps_its_group_rules(self):
        keys = ["am", "runtime", "runtime-omarchy"]
        value, api, registry = self.fixture(keys, overtaken=["am"])
        value = dict(value, contract_transition=OLDER)
        with patch("staging_cd_set.verify_transition"), \
                self.assertRaisesRegex(Refusal, SUPERSEDED) as error:
            verify_set(value, api, registry, automatic=True)
        self.assertNotIsInstance(error.exception, OvertakenByNewerFinal)

    def test_manual_dispatch_is_never_reported_as_overtaken(self):
        # A maintainer may redeploy an older app version on purpose; it is verified as chosen.
        value, api, registry = self.fixture(["web"], overtaken=["web"])
        inputs = {"deployables": "web", "mode": "release", "version": "0.10.1"}
        self.assertEqual(verify_set(value, api, registry, automatic=False, inputs=inputs), value)
        # A superseded runtime still raises, as before.
        value, api, registry = self.fixture(["runtime"], overtaken=["runtime"])
        inputs = {"deployables": "runtime", "mode": "release", "version": "0.10.2"}
        with self.assertRaises(ObsoleteSource) as error:
            verify_set(value, api, registry, automatic=False, inputs=inputs)
        self.assertNotIsInstance(error.exception, OvertakenByNewerFinal)

    def test_verify_step_reports_the_handled_disposition_and_deploys_nothing(self):
        value, api, registry = self.fixture(["web"], overtaken=["web"])
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            event = {"ref": "refs/heads/develop", "repository": {"full_name": "ChronoAIProject/cma"},
                     "after": NEWER, "deleted": False}
            (root / "event").write_text(json.dumps(event))
            env = {"GITHUB_REPOSITORY": "ChronoAIProject/cma", "GITHUB_REF": "refs/heads/develop",
                   "GITHUB_EVENT_NAME": "push", "GITHUB_SHA": NEWER, "GITHUB_EVENT_PATH": str(root / "event"),
                   "GITHUB_OUTPUT": str(root / "output"), "GITHUB_STEP_SUMMARY": str(root / "summary"),
                   "GITHUB_RUN_ID": "123", "GITHUB_RUN_ATTEMPT": "1", "RUNNER_TEMP": str(root),
                   "GH_TOKEN": "fixture", "CD_RESOLVE_MODE": "verify", "CD_RELEASE_SET": canonical(value),
                   "CD_STARTED_AT": timestamp(1000), "CD_DEPLOY_MODE": "release"}
            log = io.StringIO()
            with patch.object(resolver, "GitHub", return_value=api), \
                    patch.object(resolver, "Registry", return_value=registry), \
                    patch("staging_cd_clock.time.time", return_value=1010), \
                    patch.dict(os.environ, env, clear=True), contextlib.redirect_stdout(log):
                self.assertEqual(resolver.main(), 0)
            self.assertEqual((root / "output").read_text(), f"disposition={OVERTAKEN}\n")
            self.assertIn(f"SKIP deployment reason={OVERTAKEN} components=web", log.getvalue())
            self.assertIn(f"Deployment skipped: {OVERTAKEN} for web.", (root / "summary").read_text())
            # Any other verification failure still fails the step without a disposition.
            (root / "output").write_text("")
            api.require_current_versions.side_effect = Refusal("Component version inventory is unavailable")
            with patch.object(resolver, "GitHub", return_value=api), \
                    patch.object(resolver, "Registry", return_value=registry), \
                    patch("staging_cd_clock.time.time", return_value=1010), \
                    patch.dict(os.environ, env, clear=True), contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(resolver.main(), 1)
            self.assertEqual((root / "output").read_text(), "")


class OvertakenControllerTests(unittest.TestCase):
    """Under the set lock, a set overtaken after verification skips before any controller work."""

    def run_deploy(self, keys, *, stale, later, reach=True):
        receipts = {key: receipt(key) for key in keys}
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            env = invoke_fixtures.InvokeTests().environment(root)
            env["CD_RELEASE_SET"] = canonical(release_set(receipts))
            script = root / "staging/scripts/deploy_set.py"
            script.parent.mkdir(parents=True)
            apps = "".join(f"  {key}: {{deploy: enabled}}\n" for key in ("am", "eg", "web"))
            (script.parent.parent / "config.yaml").write_text("apps:\n" + apps)
            deployed = []

            def controller(_script, path, *_args):
                deployed.append(sorted(json.loads(path.read_text())["components"]))
                fixture_result(path)
                return 0

            def watermark(selected):
                if stale & selected.keys():
                    raise SupersededVersions(stale & selected.keys())

            chain = [NEWER, OLDER] if reach else [NEWER]
            named = []
            with patch("staging_cd_invoke.require_current_versions", side_effect=watermark), \
                    patch("staging_cd_invoke.later_final_sources", return_value=later), \
                    patch.object(invoke.DevelopHistory, "from_head", side_effect=lambda head: DevelopHistory(head, chain)):
                try:
                    code = invoke.deploy(env, Mock(), lambda _: script, controller, on_overtaken=named.extend)
                except invoke.NotAdmitted as refusal:
                    code = refusal
            return code, deployed, named

    def test_a_wholly_overtaken_set_skips_before_the_controller(self):
        for keys in (["web"], ["eg", "web"]):
            with self.subTest(keys=keys):
                code, deployed, named = self.run_deploy(keys, stale=set(keys), later={k: [NEWER] for k in keys})
                self.assertEqual((code, deployed, named), (invoke.SUPERSEDED_BY_NEWER_FINAL, [], keys))

    def test_partial_unproved_or_group_sets_deploy_or_fail_as_today(self):
        code, deployed, _ = self.run_deploy(["eg", "web"], stale={"web"}, later={"web": [NEWER], "eg": []})
        self.assertEqual((code, deployed), (0, [["eg"]]))
        for name, case in {"no-later-final": dict(keys=["web"], stale={"web"}, later={"web": []}),
                           "not-a-descendant": dict(keys=["web"], stale={"web"}, later={"web": [NEWER]}, reach=False),
                           "group-member": dict(keys=["am"], stale={"am"}, later={"am": [NEWER]})}.items():
            with self.subTest(case=name):
                code, deployed, named = self.run_deploy(**case)
                self.assertIsInstance(code, invoke.NotAdmitted)
                self.assertEqual((code.reason, deployed, named), ("authority", [], []))

    def test_main_reports_the_handled_skip_and_exits_success(self):
        def operation(_env, _masker, on_disabled, on_private_refusal, on_unverified, on_rolled_back, on_overtaken):
            on_overtaken(["web"])
            return invoke.SUPERSEDED_BY_NEWER_FINAL
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            env = {"GITHUB_STEP_SUMMARY": str(root / "summary"), "GITHUB_OUTPUT": str(root / "output")}
            log = io.StringIO()
            with patch.dict(os.environ, env, clear=True), patch.object(invoke, "deploy", side_effect=operation), \
                    contextlib.redirect_stdout(log):
                self.assertEqual(invoke.main(), 0)
            self.assertIn(f"SKIP deployment reason={OVERTAKEN} components=web", log.getvalue())
            self.assertEqual((root / "output").read_text(), f"disposition={OVERTAKEN}\n")
            self.assertIn(f"Deployment skipped: {OVERTAKEN} for web.", (root / "summary").read_text())
            self.assertNotIn("PASS_PENDING", log.getvalue())

    def test_later_final_sources_read_peeled_commits_of_higher_finals_only(self):
        receipts = {"web": receipt("web"), "eg": receipt("eg")}
        lines = ["1" * 40 + "\trefs/tags/cma-web/v0.10.1", OLDER + "\trefs/tags/cma-web/v0.10.1^{}",
                 "2" * 40 + "\trefs/tags/cma-web/v0.10.2", NEWER + "\trefs/tags/cma-web/v0.10.2^{}",
                 "3" * 40 + "\trefs/tags/cma-event-gateway/v0.10.1", OLDER + "\trefs/tags/cma-event-gateway/v0.10.1^{}"]
        runner = Mock(return_value=subprocess.CompletedProcess([], 0, "\n".join(lines), ""))
        self.assertEqual(later_final_sources(receipts, runner=runner), {"web": [NEWER], "eg": []})
        self.assertNotIn("--refs", runner.call_args.args[0])
        self.assertNotIn("GH_TOKEN", runner.call_args.kwargs["env"])
        runner.return_value.stdout = "broken\trefs/tags/cma-web/v0.10.2^{}"
        with self.assertRaisesRegex(Refusal, "malformed"):
            later_final_sources(receipts, runner=runner)


class OvertakenReportTests(unittest.TestCase):
    def env(self, web):
        return {"CD_PLAN_CHANGED": "true", "CD_ALLOCATION_RESULT": "success",
                "CD_INFRASTRUCTURE_RESULT": "success", "CD_LANES": '{"web":"web","am":"am"}',
                "CD_PUBLISH_WEB": "success", "CD_PUBLISH_AM": "success",
                "CD_PROMOTE_WEB": json.dumps(web), "CD_PROMOTE_AM": json.dumps(promotion("am"))}

    def overtaken(self):
        web = promotion("web")
        del web["outputs"]["component_result"]
        web["outputs"]["deploy_disposition"] = OVERTAKEN
        return web

    def test_an_overtaken_deployment_is_handled_but_never_a_deployed_release(self):
        rows, verified = summarize(self.env(self.overtaken()))
        self.assertIsNone(verified)
        self.assertIn(f"| web | success | admitted | success | {OVERTAKEN} | {OVERTAKEN} | transient | ", "\n".join(rows))
        self.assertIn(f"web deployment skipped: {OVERTAKEN}. A newer final's run owns this deployment; "
                      "this run has no verified web release.", rows)
        self.assertIn("| am | success | admitted | success | controller-verified | none | - | - |", rows)

    def test_overtaken_evidence_must_be_consistent(self):
        failed = self.overtaken()
        failed["outputs"]["deploy"] = "failure"
        deployed = promotion("web")
        deployed["outputs"]["deploy_disposition"] = OVERTAKEN
        unknown = dict(self.overtaken(), outputs=dict(self.overtaken()["outputs"], deploy_disposition="other"))
        for web in (failed, deployed, unknown):
            with self.subTest(web=web["outputs"]), self.assertRaises(ValueError):
                summarize(self.env(web))
        contract = self.env(promotion("web"))
        grouped = self.overtaken()
        grouped["outputs"]["admitted"] = '["am"]'
        contract.update(CD_LANES='{"am":"contract","web":"web"}', CD_PROMOTE_CONTRACT=json.dumps(grouped))
        with self.assertRaises(ValueError):
            summarize(contract)
        # A peer's failure is never hidden by a handled overtaken deployment.
        self.assertIs(summarize(dict(self.env(self.overtaken()), CD_PUBLISH_AM="failure"))[1], False)

    def admission(self, **outputs):
        value = {"admit": "success", "admitted": "[]", "failures": '{"web":"superseded-source"}',
                 "admit_disposition": OVERTAKEN}
        return {"result": "success", "outputs": dict(value, **outputs)}

    def test_an_overtaken_admission_is_handled_but_never_a_deployed_release(self):
        rows, verified = summarize(self.env(self.admission()))
        self.assertIsNone(verified)
        self.assertIn(f"| web | success | superseded-source | skipped | {OVERTAKEN} | superseded-source | transient | ",
                      "\n".join(rows))
        self.assertIn(f"web admission skipped: {OVERTAKEN}. A newer final's run owns this deployment; "
                      "this run has no verified web release.", rows)
        # Without the handled disposition the same admission stays red.
        plain = self.admission()
        del plain["outputs"]["admit_disposition"]
        self.assertIs(summarize(self.env(plain))[1], False)
        for outputs in ({"admitted": '["web"]', "failures": "{}"}, {"failures": '{"web":"publication-unqualified"}'},
                        {"deploy": "success"}, {"admit_disposition": "other"}):
            with self.subTest(outputs=outputs), self.assertRaises(ValueError):
                summarize(self.env(self.admission(**outputs)))
        self.assertIs(summarize(dict(self.env(self.admission()), CD_PUBLISH_WEB="failure"))[1], False)


if __name__ == "__main__":
    unittest.main()
