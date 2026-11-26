"""Workflow entry points preserve admission provenance and frozen set outputs."""
import contextlib
import io
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

from staging_cd_clock import timestamp
from staging_cd_history import DevelopHistory
from staging_cd_promotion import proof
from staging_cd_receipts import canonical
from staging_cd_receipts_tests import fixture_receipt, fixture_tag
from staging_cd_set import ALREADY_VERIFIED, private_document, release_set
import staging_cd_resolve as resolver
import staging_cd_set_report as report


class SetEntryTests(unittest.TestCase):
    def test_dispatch_verify_and_automatic_have_distinct_trusted_clocks(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            inputs = {"deployables": "web", "mode": "release", "version": "0.10.1"}
            event = {"ref": "refs/heads/develop", "repository": {"full_name": "ChronoAIProject/cma"}, "inputs": inputs}
            (root / "event").write_text(json.dumps(event))
            env = {"GITHUB_REPOSITORY": "ChronoAIProject/cma", "GITHUB_REF": "refs/heads/develop",
                   "GITHUB_EVENT_NAME": "workflow_dispatch", "GITHUB_SHA": "a" * 40,
                   "GITHUB_EVENT_PATH": str(root / "event"), "GITHUB_OUTPUT": str(root / "output"),
                   "GITHUB_ACTOR": "fixture-owner", "GITHUB_TRIGGERING_ACTOR": "fixture-maintainer",
                   "GITHUB_RUN_ID": "123", "GITHUB_RUN_ATTEMPT": "2", "RUNNER_TEMP": str(root),
                   "CD_RESOLVE_MODE": "dispatch", "GH_TOKEN": "fixture"}
            receipt = fixture_receipt()
            value = release_set({"web": receipt})
            api, registry = Mock(), Mock()
            api.tags.return_value = [fixture_tag(receipt)]
            api.history.return_value = DevelopHistory("a" * 40, ["a" * 40])
            run = {"id": 123, "event": "workflow_dispatch", "head_branch": "develop", "head_sha": "a" * 40,
                   "path": ".github/workflows/redeploy-staging.yml", "repository": event["repository"],
                   "created_at": timestamp(1000)}
            api.request.side_effect = lambda path: {"role_name": "maintain"} if path.startswith("collaborators/") else run
            registry.exists.return_value = True
            registry.manifest.return_value = (receipt["digest"], {})
            with patch.object(resolver, "GitHub", return_value=api), patch.object(resolver, "Registry", return_value=registry), \
                    patch("staging_cd_clock.time.time", return_value=1010), contextlib.redirect_stdout(io.StringIO()):
                with patch.dict(os.environ, env, clear=True):
                    self.assertEqual(resolver.main(), 0)
                outputs = dict(line.split("=", 1) for line in (root / "output").read_text().splitlines())
                self.assertEqual(outputs["started_at"], timestamp(1000))
                self.assertEqual(json.loads(outputs["release_set"]), value)
                verify_env = dict(env, CD_RESOLVE_MODE="verify", CD_RELEASE_SET=canonical(value),
                                  CD_STARTED_AT=outputs["started_at"], CD_DEPLOY_MODE="release")
                with patch.dict(os.environ, verify_env, clear=True):
                    self.assertEqual(resolver.main(), 0)
                with patch.dict(os.environ, dict(verify_env, CD_STARTED_AT=timestamp(1001)), clear=True):
                    self.assertEqual(resolver.main(), 1)
                event.update(after="a" * 40, deleted=False)
                run.update(event="push", path=".github/workflows/staging-cd.yml")
                (root / "event").write_text(json.dumps(event))
                matrix = {"include": [{"component": "web", "tasks": canonical([
                    {"component": "web", "mode": "resume", "receipt": receipt}])}]}
                auto = dict(env, GITHUB_EVENT_NAME="push", CD_RESOLVE_MODE="automatic", CD_PUBLICATION_MATRIX=canonical(matrix))
                previous = (root / "output").read_bytes()
                with patch.dict(os.environ, dict(auto, CD_PROMOTION_LANE="am"), clear=True):
                    self.assertEqual(resolver.main(), 1)
                self.assertEqual((root / "output").read_bytes(), previous)
                with patch.dict(os.environ, dict(auto, CD_PROMOTION_LANE="web"), clear=True):
                    self.assertEqual(resolver.main(), 0)
                outputs = dict(line.split("=", 1) for line in (root / "output").read_text().splitlines())
                self.assertEqual(outputs["started_at"], timestamp(1010))
                self.assertEqual(outputs["changed"], "true")
                artifact = root / "public-admission/admission.json"
                admission = json.loads(artifact.read_text())
                self.assertEqual(admission["release_set"], value)
                self.assertEqual(admission["failed"], {})
                self.assertEqual(admission["started_at"], outputs["started_at"])
                artifact.unlink()  # A separate invocation has a fresh runner directory.
                with patch.dict(os.environ, dict(auto, CD_PUBLICATION_MATRIX='{"include":[]}'), clear=True):
                    self.assertEqual(resolver.main(), 0)
                self.assertTrue((root / "output").read_text().endswith("changed=false\n"))
                self.assertIsNone(json.loads(artifact.read_text())["release_set"])
                artifact.unlink()
                (root / "output").unlink()
                prior = {"web": {"writer": "a" * 40, "stamp": proof(private_document(value)["components"])}}
                with patch("staging_cd_promotion.available_proofs", return_value=prior), \
                        patch.dict(os.environ, dict(auto, CD_PROMOTION_LANE="web", CD_STAGING_ENVIRONMENT="staging"), clear=True):
                    self.assertEqual(resolver.main(), 0)
                outputs = dict(line.split("=", 1) for line in (root / "output").read_text().splitlines())
                self.assertEqual((outputs["changed"], outputs["disposition"]), ("false", ALREADY_VERIFIED))
                self.assertNotIn("release_set", outputs)
                self.assertEqual(json.loads(artifact.read_text())["release_set"], value)
                with patch.dict(os.environ, dict(verify_env, GITHUB_EVENT_NAME="push", CD_DEPLOY_MODE="bootstrap"), clear=True):
                    self.assertEqual(resolver.main(), 1)

    def test_report_cannot_count_a_canceled_or_missing_expected_set_as_success(self):
        for admission, deploy, expected, code in (("success", "success", "true", 0),
                                                  ("success", "skipped", "false", 0),
                                                  ("success", "skipped", "true", 1),
                                                  ("success", "cancelled", "true", 1),
                                                  ("failure", "skipped", "true", 1),
                                                  ("skipped", "skipped", "", 0)):
            with self.subTest(deploy=deploy, expected=expected), tempfile.TemporaryDirectory() as directory:
                output, path = io.StringIO(), Path(directory) / "summary"
                env = {"CD_ADMISSION_JOB_RESULT": admission, "CD_DEPLOY_JOB_RESULT": deploy,
                       "CD_SET_EXPECTED": expected, "GITHUB_STEP_SUMMARY": str(path)}
                with patch.dict(os.environ, env, clear=True), contextlib.redirect_stdout(output):
                    self.assertEqual(report.main(), code)
                self.assertEqual("::error::" in output.getvalue(), code != 0)
                self.assertTrue(path.read_text())


if __name__ == "__main__":
    unittest.main()
