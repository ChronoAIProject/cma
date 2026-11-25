"""The selected runtime release requires both a matching pointer and an active cohort."""

import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

from staging_cd_clock import timestamp
from staging_cd_common import Refusal
import staging_cd_pointer_deploy as deploy
from staging_cd_receipts import sha256
from staging_cd_receipts_tests import fixture_receipt, fixture_runtime_receipt
from staging_cd_set import document, private_document, release_set


def observed(receipt, pods=None):
    return {"configuration": "a" * 64, "pods": pods or {"b" * 64: 0},
            "build": {"source": "e" * 40, "digest": "sha256:" + "f" * 64},
            "lanes": {"runtime": {"digest": receipt["digest"], "revision": 2}}}


def observe_promotion(_receipts, _publications, _history, _registry, read, **_kwargs):
    return read(10)["runtime_release"]


class PointerDeployTests(unittest.TestCase):
    def test_normal_controller_receives_non_runtime_and_disabled_sets(self):
        runner, mask = Mock(return_value=17), Mock()
        script, path = Path("controller.py"), Path("release.json")
        for target, enabled, keys in (("staging", "false", {"runtime"}),
                                      ("production", "true", {"am"}),
                                      ("staging", "true", {"am", "web"})):
            with self.subTest(target=target, enabled=enabled, keys=keys):
                child_env = {"LANG": "C"}
                value = deploy.run(runner, script, path, {}, dict.fromkeys(keys), "start", "release",
                                   target, {"CD_RUNTIME_POINTER_PUBLICATION_ENABLED": enabled}, child_env, 990, mask)
                self.assertEqual(value, 17)
                runner.assert_called_with(script, path, "release", "start", child_env)
        mask.assert_not_called()

    def test_production_never_falls_back_to_the_runtime_patch(self):
        runner = Mock()
        for enabled, keys in (("false", {"runtime"}), ("true", {"runtime", "runtime-omarchy"})):
            with self.subTest(enabled=enabled, keys=keys), self.assertRaisesRegex(Refusal, "authorized Agent runtime"):
                deploy.run(runner, Path("controller.py"), Path("release.json"), {}, dict.fromkeys(keys), "start",
                           "release", "production", {"CD_RUNTIME_POINTER_PUBLICATION_ENABLED": enabled}, {}, 3600, Mock())
        runner.assert_not_called()

    def test_staging_runtime_release_records_the_exact_active_image_without_app_write(self):
        receipt = fixture_runtime_receipt()
        receipts = {"runtime": receipt}
        request = private_document(release_set(receipts))
        env = {"CD_RUNTIME_POINTER_PUBLICATION_ENABLED": "true", "CD_RUNTIME_POINTER_TOKEN": "fixture-package-key",
               "GITHUB_ACTOR": "fixture-writer"}
        cohort = observed(receipt)
        runner, mask = Mock(), Mock()
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "release.json"
            with patch.object(deploy, "GitHub") as api, patch.object(deploy, "Registry"), \
                    patch.object(deploy, "cohort", return_value=cohort) as observe, \
                    patch.object(deploy, "promote", side_effect=observe_promotion) as promote:
                self.assertEqual(deploy.run(runner, Path("controller.py"), path, request, receipts, timestamp(), "release",
                                            "staging", env, {"LANG": "C"}, 990, mask), 0)
                self.assertEqual(observe.call_count, 2)
                promote.assert_called_once()
                api.return_value.require_current_versions.assert_called_once_with(receipts)
            result = document(path.with_name("component-result.json").read_text())
            self.assertEqual(result["request"], sha256(request))
            self.assertEqual(result["components"]["runtime"]["state"], "controller-verified")
            self.assertEqual(result["components"]["runtime"]["image"], request["components"]["runtime"]["image"])
            self.assertEqual(result["components"]["runtime"]["pointer"],
                             {"schema": deploy.POINTER_SCHEMA, "environment": "staging", "revision": 2,
                              "build": "e" * 40, "build_digest": "sha256:" + "f" * 64,
                              "previous": {"digest": receipt["digest"], "revision": 2}})
        runner.assert_not_called()
        mask.assert_called_once_with("fixture-package-key")

    def test_superseded_idempotent_observation_cannot_write_receipt(self):
        receipt = fixture_runtime_receipt()
        receipts = {"runtime": receipt}
        request = private_document(release_set(receipts))
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "release.json"
            with patch.object(deploy, "GitHub") as api, patch.object(deploy, "Registry") as registry, \
                    patch.object(deploy, "cohort", return_value=observed(receipt)), \
                    patch.object(deploy, "promote", side_effect=observe_promotion):
                api.return_value.require_current_versions.side_effect = Refusal("candidate superseded")
                with self.assertRaisesRegex(Refusal, "candidate superseded"):
                    deploy.execute(Path("controller.py"), path, request, receipts, timestamp(), {}, {}, limit=990)
                registry.return_value.write_runtime_pointer.assert_not_called()
            self.assertFalse(path.with_name("component-result.json").exists())

    def test_partial_manager_replacement_prevents_runtime_deployment_receipt(self):
        receipt = fixture_runtime_receipt()
        receipts = {"runtime": receipt}
        request = private_document(release_set(receipts))
        first = observed(receipt, {"b" * 64: 0, "c" * 64: 0})
        last = observed(receipt, {"b" * 64: 0, "d" * 64: 0})
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "release.json"
            with patch.object(deploy, "GitHub"), patch.object(deploy, "Registry"), \
                    patch.object(deploy, "cohort", side_effect=[first, last]), \
                    patch.object(deploy, "promote", side_effect=observe_promotion):
                with self.assertRaisesRegex(Refusal, "cohort changed"):
                    deploy.execute(Path("controller.py"), path, request, receipts, timestamp(), {}, {}, limit=990)
            self.assertFalse(path.with_name("component-result.json").exists())

    def test_mixed_release_only_sends_apps_to_the_mutating_controller(self):
        receipts = {"am": fixture_receipt(component="am"), "runtime": fixture_runtime_receipt()}
        request = private_document(release_set(receipts))
        calls = []

        def record(path, request):
            deploy.write_result(path.with_name("component-result.json"), request, {
                key: {"state": "controller-verified", "reason": "none", "image": row["image"],
                      "source_sha": row["source_sha"]} for key, row in request["components"].items()
            })
            return 0

        def rollout(_script, path, _mode, _started, _child_env):
            request = document(path.read_text())
            self.assertEqual(set(request["components"]), {"am"})
            calls.append("am")
            return record(path, request)

        def promote(_script, path, request, receipts, *_args, **_kwargs):
            self.assertEqual(set(receipts), {"runtime"})
            calls.append("runtime")
            return record(path, request)

        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "release.json"
            with patch.object(deploy, "cohort", return_value=observed(receipts["runtime"])), \
                    patch.object(deploy, "execute", side_effect=promote):
                env = {"CD_RUNTIME_POINTER_PUBLICATION_ENABLED": "true", "CD_RUNTIME_POINTER_TOKEN": "fixture"}
                self.assertEqual(deploy.run(rollout, Path("controller.py"), path, request, receipts, timestamp(),
                                            "release", "staging", env, {}, 990, Mock()), 0)
            result = document(path.with_name("component-result.json").read_text())
            self.assertEqual(result["request"], sha256(request))
            self.assertEqual(set(result["components"]), {"am", "runtime"})
        self.assertEqual(calls, ["am", "runtime"])

    def test_slow_application_rollout_cannot_renew_pointer_admission(self):
        receipts = {"am": fixture_receipt(component="am"), "runtime": fixture_runtime_receipt()}
        request = private_document(release_set(receipts))
        now = int(time.time())

        def rollout(_script, path, *_args):
            app_request = document(path.read_text())
            deploy.write_result(path.with_name("component-result.json"), app_request, {
                key: {"state": "controller-verified", "reason": "none", "image": row["image"],
                      "source_sha": row["source_sha"]} for key, row in app_request["components"].items()
            })
            return 0

        env = {"CD_RUNTIME_POINTER_PUBLICATION_ENABLED": "true", "CD_RUNTIME_POINTER_TOKEN": "fixture"}
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "release.json"
            with patch.object(deploy, "cohort", return_value=observed(receipts["runtime"])), \
                    patch.object(deploy, "execute") as execute, \
                    patch("staging_cd_clock.time.time", side_effect=[now, now + 121]):
                with self.assertRaisesRegex(Refusal, "admission budget expired"):
                    deploy.run(rollout, Path("controller.py"), path, request, receipts, timestamp(now), "release",
                               "staging", env, {}, 990, Mock())
                execute.assert_not_called()
