"""Private entry point invocation cannot disclose data or broaden credentials."""
import contextlib
import io
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

from staging_cd_common import Refusal
from staging_cd_checkout import checkout_path
from staging_cd_runtime import run_private
from staging_cd_clock import CONTROLLER_TOTAL_ENV, timestamp
from staging_cd_receipts import canonical
from staging_cd_receipts_tests import fixture_receipt
from staging_cd_set import release_set
import staging_cd_invoke as invoke


class InvokeTests(unittest.TestCase):
    def setUp(self):
        self.watermark = self.enterContext(patch.object(invoke, "require_current_versions"))

    def test_bootstrap_never_claims_recovery_and_late_success_is_uncertain(self):
        with tempfile.TemporaryDirectory() as directory:
            script = Path(directory) / "staging/scripts/deploy_set.py"
            with patch("staging_cd_clock.time.time", return_value=1010), \
                    patch.object(invoke, "run_process", return_value=10) as process:
                self.assertEqual(invoke.execute(script, Path(directory) / "input", "bootstrap", timestamp(1000), {}), 20)
                self.assertEqual(process.call_args.args[1][CONTROLLER_TOTAL_ENV], "600")
                self.assertEqual(process.call_args.args[2], 590)
                self.assertEqual(invoke.execute(script, Path(directory) / "input", "release", timestamp(1000), {}), 10)
                self.assertEqual(process.call_args.args[1][CONTROLLER_TOTAL_ENV], "990")
                self.assertEqual(process.call_args.args[2], 980)
                self.assertEqual(process.call_args.kwargs["grace_seconds"], 15)
                process.return_value = 1
                self.assertEqual(invoke.execute(script, Path(directory) / "input", "release", timestamp(1000), {}), 20)
            for private_code in (0, 20, 30, -9):
                with patch("staging_cd_clock.time.time", return_value=1980), patch.object(
                        invoke, "run_process", side_effect=invoke.ProcessTimeout("fixture", 10, private_code)):
                    self.assertEqual(invoke.execute(script, Path(directory) / "input", "release", timestamp(1000), {}),
                                     30 if private_code == 30 else 20)
            with patch("staging_cd_clock.time.time", side_effect=[1980, 1991]), \
                    patch.object(invoke, "run_process", return_value=0):
                self.assertEqual(invoke.execute(script, Path(directory) / "input", "release", timestamp(1000), {}), 20)

    def test_release_watchdog_accepts_a_result_after_the_legacy_cutoff(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            env = {CONTROLLER_TOTAL_ENV: "600"}
            with patch("staging_cd_clock.time.time", side_effect=[1010, 1670]), \
                    patch.object(invoke, "run_process", return_value=0) as child:
                self.assertEqual(invoke.execute(root / "staging/scripts/deploy_set.py", root / "input",
                                               "release", timestamp(1000), env), 0)
            self.assertEqual(child.call_args.args[1][CONTROLLER_TOTAL_ENV], "990")
            self.assertEqual(env[CONTROLLER_TOTAL_ENV], "600")

    def test_release_file_permissions_exact_schema_and_cleanup_on_unknown_outcome(self):
        with tempfile.TemporaryDirectory() as directory:
            env = self.environment(Path(directory))
            script = Path(directory) / "staging/scripts/deploy_set.py"
            paths = []
            def run(_script, path, mode, started, child_env):
                paths.append(path)
                self.assertEqual(path.stat().st_mode & 0o777, 0o600)
                self.assertEqual(path.parent.stat().st_mode & 0o777, 0o700)
                value = json.loads(path.read_text())
                self.assertEqual(set(value), {"schema", "components"})
                self.assertEqual(set(value["components"]["web"]), {"image", "source_sha", "version"})
                self.assertEqual(value["components"]["web"]["source_sha"], "a" * 40)
                self.assertNotIn("GITHUB_SHA", child_env)
                raise RuntimeError("private subprocess detail")
            code = invoke.deploy(env, Mock(), lambda _: script, run, policy_reader=lambda *_, **kw: ["web"])
            self.assertEqual(code, 20)
            self.assertTrue(paths)
            self.assertTrue(all(not path.parent.exists() for path in paths))

    def test_takeover_after_deploy_setup_blocks_controller_launch(self):
        with tempfile.TemporaryDirectory() as directory:
            env = self.environment(Path(directory))
            runner = Mock()
            self.watermark.side_effect = Refusal("Automatic deployment version has been superseded")
            with self.assertRaises(invoke.NotAdmitted):
                invoke.deploy(env, Mock(), lambda _: Path(directory), runner,
                              policy_reader=lambda *_args, **_kwargs: ["web"])
            self.watermark.assert_called_once_with({"web": fixture_receipt()})
            runner.assert_not_called()

    def test_queue_expiry_and_slow_setup_prevent_private_invocation(self):
        with tempfile.TemporaryDirectory() as directory:
            env = self.environment(Path(directory))
            env["CD_STARTED_AT"] = timestamp(1000)
            runner, reader = Mock(), Mock()
            with patch("staging_cd_clock.time.time", return_value=1120), self.assertRaises(invoke.NotAdmitted):
                invoke.deploy(env, Mock(), reader, runner)
            reader.assert_not_called()
            with patch("staging_cd_clock.time.time", side_effect=[1110, 1125]), self.assertRaises(invoke.NotAdmitted):
                invoke.deploy(env, Mock(), lambda _: Path(directory), runner,
                              policy_reader=lambda *_, **kw: ["web"])
            runner.assert_not_called()
            self.assertEqual(list(Path(directory).glob("cma-release-set-*")), [])

    def test_public_diagnostics_identify_prelaunch_refusals_and_controller_exit(self):
        marker = "private-detail-must-stay-suppressed"
        cases = (
            ("mode", 40), ("checkout", 30), ("policy", 30),
            ("credentials-format", 30), ("budget", 30),
            ("late-budget", 30), ("unexpected-checkout", 40), ("controller", 30),
            ("controller-reason", 30),
        )
        real_deploy = invoke.deploy
        for stage, code in cases:
            with self.subTest(stage=stage), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                env = self.environment(root)
                summary = root / "summary"
                env["GITHUB_STEP_SUMMARY"] = str(summary)
                reader = Mock(return_value=root / "deploy_set.py")
                policy = Mock(return_value=["web"])
                runner = Mock(return_value=30)
                budget = Mock(return_value=20)
                if stage == "mode":
                    env["CD_DEPLOY_MODE"] = marker
                elif stage == "checkout":
                    reader.side_effect = Refusal(marker)
                elif stage == "unexpected-checkout":
                    reader.side_effect = OSError(marker)
                elif stage == "policy":
                    policy.side_effect = Refusal(marker)
                elif stage == "credentials-format":
                    env["CD_NYXID_CLIENT_SECRET"] = marker + "\n"
                elif stage == "budget":
                    budget.side_effect = Refusal(marker)
                elif stage == "late-budget":
                    budget.side_effect = [20, Refusal(marker)]

                elif stage == "controller-reason":
                    def refuse(script, path, mode, started, child_env):
                        from staging_cd_receipts import sha256

                        self.assertEqual(child_env["CD_REPORT_REFUSAL"], "1")
                        packet = path.with_name("refusal-code")
                        packet.write_text(sha256(json.loads(path.read_text())) + " image-version\n")
                        packet.chmod(0o600)
                        return 30
                    runner.side_effect = refuse

                def operation(values, masker, on_disabled, on_private_refusal, on_unverified, on_rolled_back):
                    return real_deploy(values, masker, reader, runner,
                                       policy_reader=policy, on_disabled=on_disabled, on_private_refusal=on_private_refusal)

                # The runner consumes GitHub's existing mask commands. Neither
                # the new diagnostic nor the summary may carry their arguments.
                output = io.StringIO()
                with patch.dict(os.environ, env, clear=True), patch.object(invoke, "remaining", budget), \
                        patch.object(invoke, "deploy", side_effect=operation), contextlib.redirect_stdout(output):
                    self.assertEqual(invoke.main(), code)
                reason = {"late-budget": "budget", "unexpected-checkout": "checkout"}.get(stage, stage)
                reason = {"controller": "private-unreported", "controller-reason": "private-image-version"}.get(stage, reason)
                expected = "FAIL precondition=" + reason
                public = [line for line in output.getvalue().splitlines() if not line.startswith("::add-mask::")]
                self.assertEqual(public, ["FAIL", expected])
                self.assertIn(expected, summary.read_text().splitlines())
                self.assertNotIn(marker, summary.read_text())
                self.assertEqual(runner.call_count, int(stage.startswith("controller")))

    def test_private_reason_reader_refuses_wrong_binding_unsafe_files_and_unknown_bytes(self):
        from staging_cd_receipts import sha256
        from staging_cd_refusal import read

        request = {"schema": 1}
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "refusal-code"
            self.assertIsNone(read(path, request))
            for value in (b"unexpected", b"x" * 129, (sha256({}) + " clock\n").encode(),
                          (sha256(request) + " clock\nprivate-detail\n").encode()):
                with self.subTest(value=value):
                    path.write_bytes(value)
                    path.chmod(0o600)
                    self.assertIsNone(read(path, request))
            path.write_text(sha256(request) + " clock\n")
            path.chmod(0o644)
            self.assertIsNone(read(path, request))
            path.chmod(0o600)
            self.assertEqual(read(path, request), "clock")
            link = path.with_name("link")
            link.symlink_to(path)
            self.assertIsNone(read(link, request))
            path.unlink()
            os.mkfifo(path)
            self.assertIsNone(read(path, request))

    def test_unverified_packet_is_bound_allowlisted_and_used_only_for_exit_20(self):
        from staging_cd_receipts import sha256
        real_deploy = invoke.deploy
        for case, code in (("bound", 20), ("missing", 20), ("wrong-request", 20), ("unknown", 20),
                           ("trailing", 20), ("unsafe", 20), ("bad-result", 20), ("wrong-exit", 30),
                           ("late-bound", 20), ("late-missing", 20), ("late-success", 0)):
            with self.subTest(case=case), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                env = self.environment(root)
                summary = root / "summary"
                env["GITHUB_STEP_SUMMARY"] = str(summary)

                def runner(script, path, mode, started, child_env):
                    self.assertEqual(child_env["CD_REPORT_REFUSAL"], "1")
                    print("private-output-canary")
                    digest = sha256(json.loads(path.read_text())) if case != "wrong-request" else "0" * 64
                    if case == "bad-result":
                        path.with_name("component-result.json").write_text("{}")
                    value = digest + " write-timeout recovery-ownership\n"
                    if case == "unknown":
                        value = value.replace("write-timeout", "private-output-canary")
                    if case == "trailing":
                        value += "private-output-canary"
                    if case not in ("missing", "late-missing"):
                        packet = path.with_name("verification-code")
                        packet.write_text(value)
                        packet.chmod(0o644 if case == "unsafe" else 0o600)
                    return code

                def operation(values, masker, **callbacks):
                    return real_deploy(values, masker, lambda _: root, runner,
                                       policy_reader=lambda *a, **kw: ["web"], **callbacks)
                output = io.StringIO()
                with patch.dict(os.environ, env, clear=True), patch.object(invoke, "deploy", side_effect=operation), contextlib.redirect_stdout(output):
                    if case.startswith("late"):
                        clock = iter((20.0, 991.0, 991.0))
                        with patch.object(invoke, "elapsed", side_effect=lambda _: next(clock)):
                            self.assertEqual(invoke.main(), 20)
                    else:
                        self.assertEqual(invoke.main(), code)
                for text in (output.getvalue(), summary.read_text()):
                    self.assertNotIn("private-output-canary", text)
                    if code != 20 and not case.startswith("late"):
                        self.assertNotIn("FAIL verification=", text)
                    else:
                        expected = "write-timeout recovery=recovery-ownership" if case in ("bound", "late-bound") else "unreported recovery=unreported"
                        if case in ("late-missing", "late-success"):
                            expected = "phase-budget recovery=unreported"
                        if case == "bad-result":
                            expected = "result recovery=unreported"
                        self.assertIn("FAIL verification=" + expected, text)

    def test_verified_rollback_reports_only_an_allowlisted_release_token(self):
        from staging_cd_receipts import sha256
        real_deploy = invoke.deploy
        for case, expected in (("bound", "pod-restarts"), ("missing", "unreported"),
                               ("wrong-request", "unreported"), ("unknown", "unreported"),
                               ("trailing", "unreported"), ("unsafe", "unreported"),
                               ("bad-result", None), ("late", None)):
            with self.subTest(case=case), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                env = self.environment(root)
                summary = root / "summary"
                env["GITHUB_STEP_SUMMARY"] = str(summary)

                def runner(script, path, mode, started, child_env):
                    self.assertEqual(child_env["CD_REPORT_REFUSAL"], "1")
                    print("private-output-canary")
                    digest = sha256(json.loads(path.read_text())) if case != "wrong-request" else "0" * 64
                    if case == "bad-result":
                        path.with_name("component-result.json").write_text("{}")
                    value = digest + " pod-restarts unreported\n"
                    if case == "unknown":
                        value = value.replace("pod-restarts", "private-output-canary")
                    if case == "trailing":
                        value += "private-output-canary"
                    # An older controller writes no packet on exit 10.
                    if case != "missing":
                        packet = path.with_name("verification-code")
                        packet.write_text(value)
                        packet.chmod(0o644 if case == "unsafe" else 0o600)
                    return 10

                def operation(values, masker, **callbacks):
                    return real_deploy(values, masker, lambda _: root, runner,
                                       policy_reader=lambda *a, **kw: ["web"], **callbacks)
                output = io.StringIO()
                with patch.dict(os.environ, env, clear=True), patch.object(invoke, "deploy", side_effect=operation), \
                        contextlib.redirect_stdout(output):
                    if case == "late":
                        clock = iter((20.0, 991.0, 991.0))
                        with patch.object(invoke, "elapsed", side_effect=lambda _: next(clock)):
                            self.assertEqual(invoke.main(), 20)
                    else:
                        self.assertEqual(invoke.main(), 10 if expected else 20)
                public = [line for line in output.getvalue().splitlines() if not line.startswith("::add-mask::")]
                for text in (output.getvalue(), summary.read_text()):
                    self.assertNotIn("private-output-canary", text)
                    if expected is None:
                        # A late or malformed exit 10 is unverified, never a rollback claim.
                        self.assertNotIn("FAIL release=", text)
                        self.assertIn("FAIL verification=" + ("phase-budget" if case == "late" else "result")
                                      + " recovery=unreported", text)
                    else:
                        self.assertNotIn("FAIL verification=", text)
                        self.assertIn(f"FAIL release={expected} recovery=verified", text.splitlines())
                if expected is not None:
                    self.assertEqual(public, ["FAIL", f"FAIL release={expected} recovery=verified"])
                    self.assertIn(invoke.RESULTS[10], summary.read_text())

    def test_rollback_uncertainty_emits_fixed_operator_alert_and_preserves_revision_summary(self):
        with tempfile.TemporaryDirectory() as directory:
            env = self.environment(Path(directory))
            summary = Path(directory) / "summary"
            summary.write_text("Infrastructure revision: `" + "c" * 40 + "`\n")
            env["GITHUB_STEP_SUMMARY"] = str(summary)
            def operation(_env, _masker, on_disabled, on_private_refusal, on_unverified, on_rolled_back):
                return code if code in (0, 10, 20, 30, 40) else 20
            for code in (0, 10, 20, 30, 40, 1):
                output = io.StringIO()
                with patch.dict(os.environ, env, clear=True), patch.object(invoke, "deploy", side_effect=operation), contextlib.redirect_stdout(output):
                    self.assertEqual(invoke.main(), code if code in (0, 10, 20, 30, 40) else 20)
                self.assertEqual("::error::" in output.getvalue(), code in (20, 1))
                self.assertIn("Infrastructure revision:", summary.read_text())
                self.assertNotIn("fixture", output.getvalue())
            self.assertIn("NOT-YET: authenticated acceptance runs out-of-band", summary.read_text())

    def test_public_success_ends_pending_lead_check_without_forwarding_private_output(self):
        def private_success(*args, **kwargs):
            print("NOT-YET runtime_image_id reason=lead independent check required")
            print("private-origin-canary")
            os.write(2, b"private-native-canary\n")
            return 0

        with tempfile.TemporaryDirectory() as directory:
            summary = Path(directory) / "summary"
            for with_summary in (True, False):
                env = {"GITHUB_STEP_SUMMARY": str(summary)} if with_summary else {}
                output = io.StringIO()
                with patch.dict(os.environ, env, clear=True), \
                        patch.object(invoke, "deploy", side_effect=private_success), \
                        contextlib.redirect_stdout(output):
                    self.assertEqual(invoke.main(), 0)
                self.assertEqual(output.getvalue().splitlines()[-1], "PASS_PENDING_LEAD_RUNTIME_IMAGE_CHECK")
                self.assertIn("runtime_image_id", output.getvalue())
                self.assertNotIn("private-origin-canary", output.getvalue())
                self.assertNotIn("private-native-canary", output.getvalue())
                if with_summary:
                    self.assertEqual(summary.read_text().splitlines()[-1], "PASS_PENDING_LEAD_RUNTIME_IMAGE_CHECK")
                    self.assertIn("runtime_image_id", summary.read_text())
                    self.assertNotIn("private-origin-canary", summary.read_text())

    def test_failed_or_skipped_attempt_never_claims_pending_success(self):
        with tempfile.TemporaryDirectory() as directory:
            summary = Path(directory) / "summary"
            for code in (10, 20, 30, 40, invoke.UNVERIFIED_COHORT, invoke.CONTRACT_READ_FAILED, invoke.DISABLED):
                summary.write_text("")
                output = io.StringIO()
                with patch.dict(os.environ, {"GITHUB_STEP_SUMMARY": str(summary)}, clear=True), \
                        patch.object(invoke, "deploy", return_value=code), \
                        contextlib.redirect_stdout(output):
                    self.assertEqual(invoke.main(), 0 if code == invoke.DISABLED else code)
                self.assertNotIn("PASS_PENDING_LEAD_RUNTIME_IMAGE_CHECK", output.getvalue() + summary.read_text())
                if code in (invoke.UNVERIFIED_COHORT, invoke.CONTRACT_READ_FAILED):
                    self.assertIn(invoke.RESULTS[code], output.getvalue())
                    self.assertEqual(summary.read_text().splitlines()[-1], invoke.RESULTS[code])

    def environment(self, root):
        tools = root / "cma-deploy-tools"
        for name in ("home", "docker/cli-plugins", "bin", "venv/bin"):
            (tools / name).mkdir(parents=True)
        for name in ("bin/nyxid", "docker/cli-plugins/docker-buildx", "docker/config.json"):
            (tools / name).write_text("fixture")
        event = root / "event.json"
        value = {"ref": "refs/heads/develop", "after": "a" * 40, "deleted": False,
                 "repository": {"full_name": "ChronoAIProject/cma"}}
        event.write_text(json.dumps(value))
        return {"GITHUB_REPOSITORY": "ChronoAIProject/cma", "GITHUB_REF": "refs/heads/develop",
                "GITHUB_EVENT_NAME": "push", "GITHUB_SHA": "a" * 40, "GITHUB_EVENT_PATH": str(event),
                "GITHUB_WORKSPACE": str(root), "RUNNER_TEMP": str(root), "GITHUB_RUN_ID": "123", "GITHUB_RUN_ATTEMPT": "2",
                "GITHUB_OUTPUT": str(root / "step-output"),
                "CD_RELEASE_SET": canonical(release_set({"web": fixture_receipt()})),
                "CD_DEPLOY_MODE": "release", "CD_STARTED_AT": timestamp(),
                "CD_INFRA_REF": "c" * 40, "CD_NYXID_CLIENT_ID": "fixture-id", "CD_NYXID_CLIENT_SECRET": "fixture-value",
                "NYXID_CHRONO_CD_KEY": "unrelated", "GH_TOKEN": "unrelated",
                "CMA_INFRA_APP_PRIVATE_KEY": "unrelated", "CD_INFRA_TOKEN": "unrelated", "PATH": os.environ["PATH"]}

    def test_only_deploy_credential_reaches_private_script_and_failure_remains_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            env = self.environment(Path(directory))
            script = Path(directory) / "deploy_set.py"
            reader, runner = Mock(return_value=script), Mock(return_value=0)
            self.assertEqual(invoke.deploy(env, Mock(), reader, runner, policy_reader=lambda *_, **kw: ["web"]), 0)
            self.assertEqual(runner.call_args.args[0], script)
            self.assertEqual(runner.call_args.args[2:4], ("release", env["CD_STARTED_AT"]))
            self.assertFalse(runner.call_args.args[1].exists())
            child_env = runner.call_args.args[4]
            self.assertEqual(child_env["CD_NYXID_CLIENT_ID"], "fixture-id")
            self.assertEqual(child_env["CD_NYXID_CLIENT_SECRET"], "fixture-value")
            self.assertTrue(set(child_env).issubset({*invoke.IDENTITY_KEYS, *invoke.PROCESS_KEYS, "CD_NYXID_CLIENT_ID", "CD_NYXID_CLIENT_SECRET", "HOME", "DOCKER_CONFIG"}))
            self.assertEqual(child_env["HOME"], str(Path(directory) / "cma-deploy-tools/home"))
            self.assertEqual(child_env["DOCKER_CONFIG"], str(Path(directory) / "cma-deploy-tools/docker"))
            for field in ("NYXID_CHRONO_CD_KEY", "GH_TOKEN", "CD_INFRA_TOKEN", "CMA_INFRA_APP_PRIVATE_KEY", "GITHUB_OUTPUT", "GITHUB_STEP_SUMMARY"):
                self.assertNotIn(field, child_env)
            for code in (1, 2, 23, -15):
                runner.return_value = code
                self.assertEqual(invoke.deploy(env, Mock(), reader, runner, policy_reader=lambda *_, **kw: ["web"]), 20)

    def test_bad_source_component_image_or_credential_never_invokes_private_code(self):
        with tempfile.TemporaryDirectory() as directory:
            env = self.environment(Path(directory))
            reader, runner = Mock(), Mock()
            for field, value in [("GITHUB_EVENT_NAME", "pull_request"), ("GITHUB_EVENT_NAME", "workflow_dispatch"),
                                 ("GITHUB_REF", "refs/heads/main"), ("GITHUB_REPOSITORY", "Example/fork"),
                                 ("GITHUB_SHA", "main"), ("CD_DEPLOY_MODE", "bootstrap"),
                                 ("CD_RELEASE_SET", "{}")]:
                with self.subTest(field=field, value=value), self.assertRaises(Refusal):
                    invoke.deploy(dict(env, **{field: value}), Mock(), reader, runner)
            event = Path(env["GITHUB_EVENT_PATH"])
            event.write_text(json.dumps({"deleted": True}))
            with self.assertRaises(Refusal):
                invoke.deploy(env, Mock(), reader, runner)
            reader.assert_not_called()
            runner.assert_not_called()

    def test_disabled_skips_all_private_execution_even_without_a_deploy_credential(self):
        with tempfile.TemporaryDirectory() as directory:
            env = self.environment(Path(directory))
            env.pop("CD_NYXID_CLIENT_ID")
            env.pop("CD_NYXID_CLIENT_SECRET")
            script = Path(directory) / "staging/scripts/deploy_set.py"
            script.parent.mkdir(parents=True)
            config = script.parent.parent / "config.yaml"
            config.write_text("apps:\n  am: {deploy: disabled}\n  eg: {deploy: disabled}\n  web: {deploy: disabled}\n")
            runner, tools, skipped = Mock(), Mock(), Mock()
            self.assertEqual(invoke.deploy(env, Mock(), lambda _: script, runner, tools, on_disabled=skipped), invoke.DISABLED)
            skipped.assert_called_once_with()
            tools.assert_not_called()
            runner.assert_not_called()
            config.write_text(config.read_text().replace("web: {deploy: disabled}", "web: {deploy: enabled}"))
            with self.assertRaises(Refusal):
                invoke.deploy(env, Mock(), lambda _: script, runner, tools)
            runner.assert_not_called()

    def test_both_client_fields_are_required_and_legacy_key_is_never_a_fallback(self):
        with tempfile.TemporaryDirectory() as directory:
            env = self.environment(Path(directory))
            for field in ("CD_NYXID_CLIENT_ID", "CD_NYXID_CLIENT_SECRET"):
                for value in (None, "", "bad value", "bad\nvalue"):
                    runner, tools = Mock(), Mock()
                    with self.subTest(field=field, value=value), self.assertRaises(invoke.NotAdmitted):
                        invoke.deploy(dict(env, **{field: value}), Mock(), lambda _: Path(directory),
                                      runner, tools, policy_reader=lambda *_, **kw: ["web"])
                    runner.assert_not_called()
                    tools.assert_not_called()

    def test_disabled_main_exits_success_with_skip_summary_and_no_deployed_claim(self):
        def operation(_env, _masker, on_disabled, on_private_refusal, on_unverified, on_rolled_back):
            on_disabled()
            return invoke.DISABLED
        with tempfile.TemporaryDirectory() as directory:
            summary = Path(directory) / "summary"
            output = io.StringIO()
            with patch.dict(os.environ, {"GITHUB_STEP_SUMMARY": str(summary)}, clear=True), \
                    patch.object(invoke, "deploy", side_effect=operation), contextlib.redirect_stdout(output):
                self.assertEqual(invoke.main(), 0)
            self.assertIn("deploy disabled in cma-infra", output.getvalue())
            self.assertNotIn("deployed and verified", output.getvalue() + summary.read_text())

    def test_private_script_must_be_confined_clean_and_at_the_reviewed_commit(self):
        with tempfile.TemporaryDirectory() as directory:
            env = self.environment(Path(directory))
            root = checkout_path(env)
            script = root / "staging/scripts/deploy_set.py"
            script.parent.mkdir(parents=True)
            script.write_text("print('fixture')\n")
            runner = Mock(side_effect=[Mock(returncode=0, stdout="c" * 40 + "\n"), Mock(returncode=0, stdout="")])
            self.assertEqual(invoke.private_script(env, runner), script)
            for call in runner.call_args_list:
                for field in ("NYXID_CHRONO_CD_KEY", "CD_NYXID_CLIENT_ID", "CD_NYXID_CLIENT_SECRET"):
                    self.assertNotIn(field, call.kwargs["env"])
                self.assertNotIn("GH_TOKEN", call.kwargs["env"])
            for responses in ([Mock(returncode=0, stdout="d" * 40)], [Mock(returncode=1, stdout="private")],
                              [Mock(returncode=0, stdout="c" * 40), Mock(returncode=0, stdout="?? private.py")]):
                with self.assertRaises(Refusal):
                    invoke.private_script(env, Mock(side_effect=responses))
            script.unlink()
            outside = Path(directory) / "outside.py"
            outside.write_text("print('fixture')\n")
            script.symlink_to(outside)
            with self.assertRaises(Refusal):
                invoke.private_script(env, Mock())
            with self.assertRaises(Refusal):
                invoke.private_script(dict(env, CD_INFRA_REF="main"), Mock())

    def test_real_child_output_is_suppressed_and_timeout_stops_the_process(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            script = root / "staging/scripts/deploy_set.py"
            script.parent.mkdir(parents=True)
            observed = root / "observed.json"
            script.write_text("import json, os, sys\n"
                              "print('private stdout'); os.write(2, b'private stderr')\n"
                              "open('observed.json', 'w').write(json.dumps({'args': sys.argv[1:], 'keys': sorted(os.environ)}))\n")
            env = self.environment(root)
            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                code = run_private(lambda masker: invoke.deploy(env, masker, lambda _: script, policy_reader=lambda *_, **kw: ["web"]) == 0, None)
            self.assertEqual(code, 0)
            self.assertNotIn("private stdout", output.getvalue())
            value = json.loads(observed.read_text())
            self.assertEqual(value["args"][::2], ["--release-set", "--mode", "--started-at"])
            self.assertEqual(value["args"][3:], ["release", "--started-at", env["CD_STARTED_AT"]])
            self.assertFalse(Path(value["args"][1]).exists())
            self.assertNotIn("GH_TOKEN", value["keys"])
            self.assertNotIn("NYXID_CHRONO_CD_KEY", " ".join(value["args"]))
            script.write_text("import time\ntime.sleep(30)\n")
            self.assertEqual(invoke.execute(script, root / "release.json", "release", timestamp(), {"PATH": env["PATH"]}, timeout=0.05), 20)


if __name__ == "__main__":
    unittest.main()
