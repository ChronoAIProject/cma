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
import staging_cd_invoke as invoke


class InvokeTests(unittest.TestCase):
    def test_rollback_uncertainty_emits_fixed_operator_alert_and_preserves_revision_summary(self):
        with tempfile.TemporaryDirectory() as directory:
            env = self.environment(Path(directory))
            summary = Path(directory) / "summary"
            summary.write_text("Infrastructure revision: `" + "c" * 40 + "`\n")
            env["GITHUB_STEP_SUMMARY"] = str(summary)
            def operation(_env, _masker, runner):
                return runner("fixture", "web", "fixture", {}) == 0
            for code in (0, 10, 20, 30, 40, 1):
                output = io.StringIO()
                with patch.dict(os.environ, env, clear=True), patch.object(invoke, "deploy", side_effect=operation), \
                        patch.object(invoke, "execute", return_value=code), contextlib.redirect_stdout(output):
                    self.assertEqual(invoke.main(), 0 if code == 0 else 1)
                self.assertEqual("::error::" in output.getvalue(), code in (20, 1))
                self.assertIn("Infrastructure revision:", summary.read_text())
                self.assertNotIn("fixture", output.getvalue())

    def environment(self, root):
        event = root / "event.json"
        value = {"ref": "refs/heads/develop", "after": "a" * 40, "deleted": False,
                 "repository": {"full_name": "ChronoAIProject/cma"}}
        event.write_text(json.dumps(value))
        return {"GITHUB_REPOSITORY": "ChronoAIProject/cma", "GITHUB_REF": "refs/heads/develop",
                "GITHUB_EVENT_NAME": "push", "GITHUB_SHA": "a" * 40, "GITHUB_EVENT_PATH": str(event),
                "GITHUB_WORKSPACE": str(root), "GITHUB_RUN_ID": "123", "GITHUB_RUN_ATTEMPT": "2",
                "CD_SOURCE_SHA": "d" * 40, "CD_COMPONENT": "web", "CD_IMAGE_REFERENCE": "ghcr.io/chronoaiproject/cma-web@sha256:" + "b" * 64,
                "CD_INFRA_REF": "c" * 40, "NYXID_CHRONO_CD_KEY": "fixture", "GH_TOKEN": "unrelated",
                "CMA_INFRA_APP_PRIVATE_KEY": "unrelated", "CD_INFRA_TOKEN": "unrelated", "PATH": os.environ["PATH"]}

    def test_only_deploy_credential_reaches_private_script_and_failure_remains_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            env = self.environment(Path(directory))
            script = Path(directory) / "deploy.py"
            reader, runner = Mock(return_value=script), Mock(return_value=0)
            self.assertTrue(invoke.deploy(env, Mock(), reader, runner))
            self.assertEqual(runner.call_args.args[:3], (script, "web", env["CD_IMAGE_REFERENCE"]))
            child_env = runner.call_args.args[3]
            self.assertEqual(child_env["NYXID_CHRONO_CD_KEY"], "fixture")
            self.assertTrue(set(child_env).issubset({*invoke.IDENTITY_KEYS, *invoke.PROCESS_KEYS, "NYXID_CHRONO_CD_KEY"}))
            for field in ("GH_TOKEN", "CD_INFRA_TOKEN", "CMA_INFRA_APP_PRIVATE_KEY"):
                self.assertNotIn(field, child_env)
            for code in (1, 2, 23, -15):
                runner.return_value = code
                self.assertFalse(invoke.deploy(env, Mock(), reader, runner))

    def test_bad_source_component_image_or_credential_never_invokes_private_code(self):
        with tempfile.TemporaryDirectory() as directory:
            env = self.environment(Path(directory))
            reader, runner = Mock(), Mock()
            for field, value in [("GITHUB_EVENT_NAME", "pull_request"), ("GITHUB_EVENT_NAME", "workflow_dispatch"),
                                 ("GITHUB_REF", "refs/heads/main"), ("GITHUB_REPOSITORY", "Example/fork"),
                                 ("GITHUB_SHA", "main"), ("CD_COMPONENT", "runtime"),
                                 ("CD_IMAGE_REFERENCE", "ghcr.io/chronoaiproject/cma-web:0.10.1"),
                                 ("CD_IMAGE_REFERENCE", "ghcr.io/chronoaiproject/cma-agent-manager@sha256:" + "b" * 64),
                                 ("NYXID_CHRONO_CD_KEY", ""), ("NYXID_CHRONO_CD_KEY", "fixture\n")]:
                with self.subTest(field=field, value=value), self.assertRaises(Refusal):
                    invoke.deploy(dict(env, **{field: value}), Mock(), reader, runner)
            event = Path(env["GITHUB_EVENT_PATH"])
            event.write_text(json.dumps({"deleted": True}))
            with self.assertRaises(Refusal):
                invoke.deploy(env, Mock(), reader, runner)
            reader.assert_not_called()
            runner.assert_not_called()

    def test_private_script_must_be_confined_clean_and_at_the_reviewed_commit(self):
        with tempfile.TemporaryDirectory() as directory:
            env = self.environment(Path(directory))
            root = checkout_path(env)
            script = root / "staging/scripts/deploy.py"
            script.parent.mkdir(parents=True)
            script.write_text("print('fixture')\n")
            runner = Mock(side_effect=[Mock(returncode=0, stdout="c" * 40 + "\n"), Mock(returncode=0, stdout="")])
            self.assertEqual(invoke.private_script(env, runner), script)
            for call in runner.call_args_list:
                self.assertNotIn("NYXID_CHRONO_CD_KEY", call.kwargs["env"])
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
            script = root / "staging/scripts/deploy.py"
            script.parent.mkdir(parents=True)
            observed = root / "observed.json"
            script.write_text("import json, os, sys\n"
                              "print('private stdout'); os.write(2, b'private stderr')\n"
                              "open('observed.json', 'w').write(json.dumps({'args': sys.argv[1:], 'keys': sorted(os.environ)}))\n")
            env = self.environment(root)
            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                code = run_private(lambda masker: invoke.deploy(env, masker, lambda _: script), None)
            self.assertEqual(code, 0)
            self.assertNotIn("private stdout", output.getvalue())
            value = json.loads(observed.read_text())
            self.assertEqual(value["args"], ["web", env["CD_IMAGE_REFERENCE"]])
            self.assertNotIn("GH_TOKEN", value["keys"])
            self.assertNotIn("NYXID_CHRONO_CD_KEY", " ".join(value["args"]))
            script.write_text("import time\ntime.sleep(30)\n")
            self.assertEqual(invoke.execute(script, "web", env["CD_IMAGE_REFERENCE"], {"PATH": env["PATH"]}, timeout=0.05), 1)


if __name__ == "__main__":
    unittest.main()
