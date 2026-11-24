"""No partial publication, unscanned companion, mutable signal or output leak."""
import contextlib
import copy
import io
import json
import os
from pathlib import Path
import secrets
import tempfile
import unittest
from unittest.mock import Mock, patch

import staging_cd_publication_signal as signal
from staging_cd_common import Refusal
from staging_cd_history import DevelopHistory
from staging_cd_receipts import canonical
from staging_cd_receipts_tests import fixture_receipt, fixture_runtime_receipt, fixture_tag


class SignalTests(unittest.TestCase):
    def setUp(self):
        self.receipts = {key: fixture_receipt(component=key) for key in ("eg", "web")}
        self.receipts["runtime"] = fixture_runtime_receipt()
        self.api, self.registry = Mock(), Mock()
        self.api.tags.side_effect = lambda key: [fixture_tag(self.receipts[key])]
        self.api.history.return_value = DevelopHistory("a" * 40, ["a" * 40])
        self.registry.exists.return_value = True
        self.registry.manifest.return_value = ("sha256:" + "d" * 64, {})
        self.env = {"GITHUB_SHA": "a" * 40, "GITHUB_RUN_ID": "100", "GITHUB_RUN_ATTEMPT": "1"}
        self.env.update({f"CD_FINALIZED_{key.upper()}": "success" for key in self.receipts})
        self.env.update({f"CD_PUBLISH_{key.upper()}": "success" for key in self.receipts})
        self.env.update(CD_PUBLISH_AM="skipped", CD_FINALIZED_AM="")
        self.matrix = {"include": [{"component": key, "tasks": canonical([
            {"component": key, "mode": "resume", "receipt": value}])}
            for key, value in self.receipts.items()]}

    def freeze(self):
        return signal.freeze(self.matrix, self.env, self.api, self.registry)

    def test_every_component_is_bound_before_complete_notification(self):
        value = self.freeze()
        self.assertEqual(value["receipts"], self.receipts)
        self.assertEqual(signal.validate(value), value)
        self.assertEqual(value["repository"], "ChronoAIProject/cma")
        self.api.request.assert_not_called()
        self.assertEqual(self.freeze(), value)

    def test_companion_failure_refuses_notification_even_when_deployables_published(self):
        self.api.tags.side_effect = lambda key: [] if key == "runtime" else [fixture_tag(self.receipts[key])]
        with self.assertRaises(Refusal):
            self.freeze()

    def test_failed_publication_or_finalization_refuses_even_when_the_receipt_exists(self):
        for key in ("CD_PUBLISH_RUNTIME", "CD_FINALIZED_RUNTIME"):
            with self.subTest(key=key), patch.dict(self.env, {key: "failure"}):
                with self.assertRaisesRegex(Refusal, "did not publish and finalize"):
                    self.freeze()
        self.api.tags.assert_not_called()
        self.assertEqual(self.freeze()["receipts"], self.receipts)

    def test_digest_fields_extra_keys_empty_or_reordered_receipts_cannot_change(self):
        value = self.freeze()
        for change in ({"source": "b" * 40}, {"run_id": True}, {"receipts": {}},
                       {"receipts": {"web": self.receipts["eg"]}}, {"unknown": "field"},
                       {"repository": "example/fork"}, {"id": "f" * 64}):
            with self.subTest(change=change), self.assertRaises(Refusal):
                signal.validate(dict(value, **change))
        bad = copy.deepcopy(value)
        bad["receipts"]["runtime"]["scan"]["complete"] = False
        with self.assertRaises(Refusal):
            signal.validate(bad)

    def test_sender_token_is_only_in_captured_child_environment(self):
        value = self.freeze()
        token = secrets.token_urlsafe(32)
        env = dict(self.env, PUBLICATION_CONSUMER="example/consumer", PUBLICATION_SIGNAL_TOKEN=token,
                   UNRELATED_CREDENTIAL=secrets.token_urlsafe(32), PATH="/fixture/bin")
        runner = Mock(return_value=Mock(returncode=0))
        signal.send(value, env, runner)
        args, kwargs = runner.call_args
        self.assertNotIn(token, str(args))
        self.assertNotIn(token, kwargs["input"])
        self.assertEqual(kwargs["env"]["GH_TOKEN"], token)
        self.assertNotIn("UNRELATED_CREDENTIAL", kwargs["env"])
        self.assertTrue(kwargs["capture_output"])
        self.assertEqual(json.loads(kwargs["input"])["client_payload"], value)
        runner.return_value.returncode = 1
        with self.assertRaisesRegex(Refusal, "delivery failed"):
            signal.send(value, env, runner)

    def test_destination_and_credential_validation_fail_before_network(self):
        value = self.freeze()
        for destination in (None, "https://foreign.invalid", "x/y/../z", "chronoaiPROJECT/CMA", "example/.."):
            runner = Mock()
            with self.assertRaises(Refusal):
                signal.send(value, {"PUBLICATION_CONSUMER": destination}, runner)
            runner.assert_not_called()
        with self.assertRaises(Refusal):
            signal.send(value, {"PUBLICATION_CONSUMER": "example/consumer"}, Mock())

    def test_cli_freeze_and_send_bind_current_event_and_suppress_private_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            event = root / "event.json"
            event.write_text(canonical({"ref": "refs/heads/develop", "after": "a" * 40,
                "deleted": False, "repository": {"full_name": "ChronoAIProject/cma"}}))
            env = dict(self.env, RUNNER_TEMP=directory, GITHUB_REPOSITORY="ChronoAIProject/cma",
                       GITHUB_REF="refs/heads/develop", GITHUB_EVENT_NAME="push",
                       GITHUB_EVENT_PATH=str(event), CD_PUBLICATION_MATRIX=canonical(self.matrix))
            with patch.dict(os.environ, env, clear=True), patch.object(signal, "GitHub", return_value=self.api), \
                    patch.object(signal, "Registry", return_value=self.registry), patch("sys.argv", ["signal", "freeze"]), \
                    patch.object(signal, "local_tag_reader", return_value=Mock()) as local_reader:
                self.assertEqual(signal.main(), 0)
                local_reader.assert_called_once_with(Path.cwd())
                signal.GitHub.assert_called_once_with(None, local_tags=local_reader.return_value)
                self.assertEqual(signal.main(), 1)  # Immutable local artifact.
            value = json.loads((root / "publication-signal.json").read_text())
            with patch.dict(os.environ, env, clear=True), patch("sys.argv", ["signal", "send"]), \
                    patch.object(signal, "send") as sender:
                self.assertEqual(signal.main(), 0)
                sender.assert_called_once_with(value, os.environ)
                with patch.dict(os.environ, {"GITHUB_RUN_ATTEMPT": "2"}):
                    self.assertEqual(signal.main(), 1)
                sender.assert_called_once()
                sender.side_effect = ValueError("synthetic-private-canary")
                output = io.StringIO()
                with contextlib.redirect_stdout(output):
                    self.assertEqual(signal.main(), 1)
                self.assertNotIn("synthetic-private-canary", output.getvalue())
            with patch.dict(os.environ, env, clear=True), patch("sys.argv", ["signal", "unknown"]):
                self.assertEqual(signal.main(), 1)


if __name__ == "__main__":
    unittest.main()
