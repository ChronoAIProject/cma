"""The manual staging publication plan binds a single immutable public run."""
import base64
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

from staging_cd_common import Refusal
from staging_cd_publication_plan import INPUT_FILE, REPORT_FILE, execute, private_policy, selector, write


class PublicationPlanTests(unittest.TestCase):
    def test_manual_staging_selector_refuses_wrong_target_attempt_and_mode(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "event.json"
            env = {"GITHUB_REPOSITORY": "ChronoAIProject/cma", "GITHUB_REF": "refs/heads/develop",
                   "GITHUB_EVENT_NAME": "workflow_dispatch", "GITHUB_SHA": "a" * 40,
                   "GITHUB_EVENT_PATH": str(path)}
            inputs = {"deployables": "am,eg,web", "mode": "publication-plan", "publication_run": "101",
                      "publication_attempt": "1", "publication_signal": "b" * 64}
            event = {"ref": "develop", "repository": {"full_name": "ChronoAIProject/cma"}, "inputs": inputs}
            path.write_text(json.dumps(event))
            self.assertEqual(selector(env), (101, 1, "b" * 64))
            with self.assertRaises(Refusal):
                selector(dict(env, CD_TARGET="production"))
            for change in ({"publication_attempt": "2"}, {"mode": "release"}, {"publication_run": "1/2"}):
                path.write_text(json.dumps(dict(event, inputs=dict(inputs, **change))))
                with self.assertRaises(Refusal):
                    selector(env)

    def test_policy_fetch_reads_data_from_only_the_pinned_private_file(self):
        reader = Mock()
        reader.return_value.return_value = {
            "type": "file", "encoding": "base64", "content": base64.b64encode(b'{"fixture":true}').decode(),
        }
        env = {"CD_PUBLICATION_POLICY_REF": "c" * 40, "CD_INFRA_TOKEN": "fixture"}
        self.assertEqual(private_policy(env, reader), b'{"fixture":true}')
        reader.return_value.assert_called_once_with(
            "repos/ChronoAIProject/cma-infra/contents/staging/publication-policy.json?ref=" + "c" * 40
        )
        with self.assertRaises(Refusal):
            private_policy(dict(env, CD_PUBLICATION_POLICY_REF="main"), reader)

    def test_execute_checks_the_signal_before_opening_the_staging_client(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = (101, 1, "b" * 64)
            signal = {"run_id": 101, "run_attempt": 1, "id": target[2], "source": "a" * 40,
                      "receipts": {"eg": {}}}
            write(root / INPUT_FILE, signal)
            contract = SimpleNamespace(document=json.loads, read_policy=Mock(), validate=lambda value, _: value)
            private = Mock()
            private.releases.return_value = [{"eg": {"image": "fixture"}}]
            private.Coordinator.return_value.execute.return_value = 0
            private.plan_comparison.return_value = [{"component": "eg", "change": False}]
            with patch.dict("sys.modules", {"publication_contract": contract}), \
                 patch("staging_cd_publication_plan.tool_environment", return_value={}):
                with self.assertRaises(Refusal):
                    execute({}, (102, 1, target[2]), private, root)
                private.StagingChronoCD.assert_not_called()
                execute({}, target, private, root)
            private.Coordinator.return_value.execute.assert_called_once()
            self.assertTrue(private.Coordinator.return_value.execute.call_args.kwargs["read_only"])
            report = json.loads((root / REPORT_FILE).read_bytes())
            self.assertEqual((report["mutations"], report["resource_methods"], report["signal_id"]),
                             (0, ["GET"], target[2]))


if __name__ == "__main__":
    unittest.main()
