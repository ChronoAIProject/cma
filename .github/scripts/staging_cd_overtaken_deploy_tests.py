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

import staging_cd_resolve as resolver
from staging_cd_clock import timestamp
from staging_cd_common import Refusal
from staging_cd_component_report import summarize
from staging_cd_component_report_tests import promotion
from staging_cd_history import DevelopHistory
from staging_cd_receipts import canonical
from staging_cd_receipts_tests import fixture_receipt, fixture_runtime_receipt, fixture_tag
from staging_cd_resolve import ObsoleteSource
from staging_cd_set import OVERTAKEN, OvertakenByNewerFinal, release_set, verify_set
from staging_cd_watermark import SUPERSEDED, require_current_versions

OLDER, NEWER = "a" * 40, "b" * 40


def receipt(component, source=OLDER, version="0.10.1"):
    previous = "0.10.0" if version == "0.10.1" else "0.10.1"
    make = fixture_runtime_receipt if component.startswith("runtime") else fixture_receipt
    return make(component=component, source=source, version=version, previous=previous)


class OvertakenDeploymentTests(unittest.TestCase):
    def fixture(self, keys, *, overtaken, listed=None):
        """An admitted older set; `overtaken` members have a final at a newer develop source."""
        receipts = {key: receipt(key) for key in keys}
        newer = {key: receipt(key, NEWER, "0.10.2") for key in overtaken}
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
        inputs = {"deployables": "runtime", "mode": "release", "version": "0.10.1"}
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
        self.assertIn(f"| web | success | admitted | success | {OVERTAKEN} |", rows)
        self.assertIn(f"web deployment skipped: {OVERTAKEN}. A newer final's run owns this deployment; "
                      "this run has no verified web release.", rows)
        self.assertIn("| am | success | admitted | success | controller-verified |", rows)

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


if __name__ == "__main__":
    unittest.main()
