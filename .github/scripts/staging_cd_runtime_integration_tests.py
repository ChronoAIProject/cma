"""Runtime evidence and historical receipts survive qualified caller integration."""
import copy
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock

from staging_cd_common import Refusal
from staging_cd_invoke import deploy, NotAdmitted
import staging_cd_invoke_tests
from staging_cd_receipts import annotation, canonical, parse_annotation
from staging_cd_receipts_tests import fixture_receipt, fixture_runtime_receipt
from staging_cd_set import private_document, release_set
from staging_cd_targets import APP_TARGETS, RUNTIME_TARGETS


class RuntimeIntegrationTests(unittest.TestCase):
    def test_historical_receipts_roundtrip_without_inventing_runtime_evidence(self):
        for component in APP_TARGETS | RUNTIME_TARGETS:
            with self.subTest(component=component):
                old = fixture_receipt(component=component)
                encoded = canonical(old) + "\n"
                restored = parse_annotation(encoded)
                self.assertEqual(restored, old)
                self.assertEqual(annotation(restored), encoded)
                self.assertNotIn("runtime_smoke", restored)
                if component in APP_TARGETS:
                    payload = private_document(release_set({component: restored}))
                    self.assertEqual(set(payload["components"][component]), {"image", "source_sha", "version"})
                else:
                    with self.assertRaises(Refusal):
                        release_set({component: restored})

    def test_runtime_qualification_survives_annotation_and_qualified_private_call(self):
        for component in RUNTIME_TARGETS:
            for mixed in (False, True):
                with self.subTest(component=component, mixed=mixed), tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    receipt = parse_annotation(annotation(fixture_runtime_receipt(component=component)))
                    receipts = {component: receipt}
                    if mixed:
                        receipts["am"] = fixture_receipt(component="am")
                    env = staging_cd_invoke_tests.InvokeTests().environment(root)
                    env["CD_RELEASE_SET"] = canonical(release_set(receipts))
                    script = root / "staging/scripts/deploy_set.py"
                    script.parent.mkdir(parents=True)
                    (script.parent.parent / "config.yaml").write_text(
                        "apps:\n  am: {deploy: enabled}\n  eg: {deploy: enabled}\n  web: {deploy: enabled}\n")
                    paths = []
                    def run(_script, path, _mode, _started, child_env):
                        paths.append(path)
                        payload = json.loads(path.read_text())
                        self.assertEqual(set(payload["components"]), set(receipts))
                        self.assertEqual(payload["components"][component]["runtime_smoke"], receipt["runtime_smoke"])
                        self.assertEqual(payload["components"][component]["image"],
                                         receipt["repository"] + "@" + receipt["digest"])
                        for field in ("CD_NYXID_CLIENT_ID", "CD_NYXID_CLIENT_SECRET"):
                            self.assertEqual(child_env[field], env[field])
                            self.assertNotIn(env[field], path.read_text())
                        for field in ("GH_TOKEN", "NYXID_CHRONO_CD_KEY", "GITHUB_OUTPUT"):
                            self.assertNotIn(field, child_env)
                        if mixed:
                            self.assertNotIn("runtime_smoke", payload["components"]["am"])
                        return 0
                    runner = Mock(side_effect=run)
                    self.assertEqual(deploy(env, Mock(), lambda _: script, runner), 0)
                    runner.assert_called_once()
                    self.assertTrue(all(not path.exists() for path in paths))

    def test_missing_or_mismatched_evidence_refuses_before_private_execution(self):
        for component in RUNTIME_TARGETS:
            for failure in ("historical", "source", "digest", "archive", "passed"):
                with self.subTest(component=component, failure=failure), tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    receipt = copy.deepcopy(fixture_runtime_receipt(component=component))
                    if failure == "historical":
                        receipt.pop("runtime_smoke")
                    else:
                        receipt["runtime_smoke"][failure] = False if failure == "passed" else "mismatch"
                    env = staging_cd_invoke_tests.InvokeTests().environment(root)
                    # Exercise the external input boundary, not fixture construction.
                    env["CD_RELEASE_SET"] = canonical({"schema": "cma-deployment-set-v1", "receipts": {component: receipt}})
                    reader, runner = Mock(), Mock()
                    with self.assertRaises(Refusal):
                        deploy(env, Mock(), reader, runner)
                    reader.assert_not_called()
                    runner.assert_not_called()

    def test_runtime_release_requires_both_qualified_credentials(self):
        for component in RUNTIME_TARGETS:
            for field in ("CD_NYXID_CLIENT_ID", "CD_NYXID_CLIENT_SECRET"):
                with self.subTest(component=component, field=field), tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    env = staging_cd_invoke_tests.InvokeTests().environment(root)
                    env["CD_RELEASE_SET"] = canonical(release_set({component: fixture_runtime_receipt(component=component)}))
                    env.pop(field)
                    runner = Mock()
                    with self.assertRaises(NotAdmitted):
                        deploy(env, Mock(), lambda _: root, runner,
                               policy_reader=lambda *_, **__: [component])
                    runner.assert_not_called()
