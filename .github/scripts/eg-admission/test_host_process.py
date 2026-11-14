"""Failure retention tests: all Docker/process calls are inert test doubles."""
import contextlib
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch

import host_process


class HostProcessTests(unittest.TestCase):
    def run_fixture(self, root, capture_failure=False, custody=False):
        application, binary, output = root / "application", root / "binary", root / "output"
        application.write_bytes(b"inert application binding")
        binary.write_bytes(b"inert test binding")
        process = Mock(returncode=101)
        process.communicate.return_value = (b"the same admitted event must remain accepted\n", None)
        group = Mock()
        group.cleanup.return_value = {"process_group_cleanup": not custody}
        def spawn(_arguments, **options):
            env = options["env"]
            child = Path(env["TMPDIR"]) / "cmaeg-runner-inert"
            child.mkdir()
            metadata = {"replicas": 1, "enabled": False, "ordinal": 1, "joined": True,
                        "process_sha256": "a" * 64, "executable_sha256": env["CMAEG_HOST_APPLICATION_SHA256"]}
            (child / "capture.json").write_text(json.dumps(metadata))
            if not capture_failure:
                (child / "trace.log").write_bytes(b"private log must not be published\n")
            return process
        group.spawn.side_effect = spawn
        with patch.object(host_process, "OwnedGroup", return_value=group), \
                patch.object(host_process, "command", return_value="127.0.0.1:27017"), \
                patch.object(host_process.subprocess, "run", return_value=Mock(returncode=0)), \
                patch.object(host_process, "cleanup_container") as cleanup, \
                contextlib.redirect_stdout(io.StringIO()):
            code = host_process.run(binary, application, output, "runner_binary::replicas::")
        return code, json.loads((output / "mongo-process-results.json").read_text()), output, cleanup

    def test_primary_refusal_survives_collection_with_private_traces_retained(self):
        with tempfile.TemporaryDirectory() as root:
            code, result, output, cleanup = self.run_fixture(Path(root))
            self.assertEqual(code, 2)
            self.assertEqual(result["status"], "FAIL")
            self.assertEqual(result["exit_code"], 101)
            self.assertEqual(result["failure_codes"], ["cross_ingress_not_accepted"])
            self.assertFalse(result["cleanup"])
            self.assertTrue(result["fixture_directory_retained"])
            self.assertTrue(result["private_failed_trace_retained"])
            self.assertTrue(result["container_cleanup"])
            cleanup.assert_called_once()
            self.assertNotIn("private log", (output / "evidence/n1-false-1.json").read_text())

    def test_capture_failure_keeps_private_directory_and_original_failure(self):
        with tempfile.TemporaryDirectory() as root:
            _, result, output, cleanup = self.run_fixture(Path(root), capture_failure=True)
            self.assertEqual(result["primary_status"], "FAIL")
            self.assertEqual(result["status"], "BLOCKED")
            self.assertEqual(result["code"], "host_capture_incomplete")
            self.assertEqual(result["failure_codes"], ["cross_ingress_not_accepted"])
            self.assertTrue(result["fixture_directory_retained"])
            self.assertFalse(result["cleanup"])
            self.assertTrue(list(output.glob("process-fixture-*")))
            cleanup.assert_called_once()
            self.assertTrue(result["container_cleanup"])

    def test_unconfirmed_group_cannot_collect_or_dispose(self):
        with tempfile.TemporaryDirectory() as root:
            _, result, output, cleanup = self.run_fixture(Path(root), custody=True)
            self.assertEqual(result["primary_status"], "FAIL")
            self.assertFalse(result["cleanup"])
            self.assertTrue(result["fixture_directory_retained"])
            self.assertNotIn("host_capture", result)
            self.assertFalse((output / "evidence/capture-receipt.json").exists())
            cleanup.assert_not_called()


if __name__ == "__main__":
    unittest.main()
