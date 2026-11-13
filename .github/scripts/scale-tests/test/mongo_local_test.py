"""Exercise fixture isolation/cleanup paths without contacting Docker."""
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import Mock, patch


SPEC = importlib.util.spec_from_file_location(
    "mongo_local", Path(__file__).resolve().parents[1] / "mongo-local.py")
RUNNER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(RUNNER)


class FixtureRunnerTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(dir=Path(__file__).parent)
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.binary = self.root / "compiled-fixture"
        self.binary.write_bytes(b"synthetic-binary")
        self.calls = []
        self.process = Mock(returncode=0, pid=12345)
        self.process.communicate.return_value = (b"test result: ok. 1 passed; 0 failed;", None)

    def external(self, arguments, **_):
        self.calls.append(arguments)
        output = b""
        if arguments[:2] == ["docker", "port"]:
            output = b"127.0.0.1:19000"
        if arguments[:2] == ["docker", "inspect"]:
            output = b"a" * 24
        return subprocess.CompletedProcess(arguments, 0, output, b"")

    def execute(self, external=None):
        with patch.object(RUNNER.subprocess, "run", side_effect=external or self.external), \
             patch.object(RUNNER.subprocess, "Popen", return_value=self.process), \
             patch.object(RUNNER.secrets, "token_hex", side_effect=lambda n: "a" * (2 * n)), \
             patch("sys.stdout", new_callable=io.StringIO):
            code = RUNNER.run(self.binary, self.root / "result", "runner_binary::test")
        return code, json.loads((self.root / "result/mongo-process-results.json").read_text())

    def test_success_has_owned_cleanup_exact_count_and_no_credential_argv(self):
        code, result = self.execute()
        self.assertEqual(code, 0)
        self.assertTrue(result["cleanup"])
        self.assertEqual(result["tests_passed"], 1)
        self.assertTrue(any(args[:2] == ["docker", "rm"] for args in self.calls))
        self.assertNotIn("a" * 48, json.dumps(self.calls))
        self.assertEqual(result["mongo_limits"]["db_tmpfs_mib"], 1024)

    def test_zero_or_multiple_selected_tests_cannot_pass(self):
        self.process.communicate.return_value = (b"test result: ok. 0 passed; 0 failed;", None)
        code, result = self.execute()
        self.assertEqual(code, 2)
        self.assertEqual(result["status"], "FAIL")
        self.assertTrue(result["cleanup"])

    def test_failure_keeps_only_numeric_and_fixed_diagnostics(self):
        self.process.returncode = 101
        self.process.communicate.return_value = (
            b"private-payload same admitted event must remain accepted left: 409 right: 202 "
            b"fixture_diagnostic stage=index code=14031 kind=disk_space", None)
        _, result = self.execute()
        self.assertEqual(result["numeric_assertions"], [409, 202])
        self.assertEqual(result["migration_diagnostics"][0]["code"], 14031)
        self.assertEqual(result["failure_codes"], ["cross_ingress_not_accepted"])
        self.assertNotIn("private-payload", json.dumps(result))

    def test_partial_container_creation_is_cleaned_after_run_fails(self):
        def fail_run(arguments, **options):
            if arguments[:2] == ["docker", "run"]:
                return subprocess.CompletedProcess(arguments, 1, b"", b"private-daemon-error")
            return self.external(arguments, **options)
        _, result = self.execute(fail_run)
        self.assertEqual(result["status"], "BLOCKED")
        self.assertTrue(result["cleanup"])
        self.assertTrue(any(args[:2] == ["docker", "rm"] for args in self.calls))

    def test_cleanup_does_not_delete_an_unowned_container(self):
        def wrong_owner(arguments, **options):
            if arguments[:2] == ["docker", "inspect"]:
                return subprocess.CompletedProcess(arguments, 0, b"different-owner", b"")
            return self.external(arguments, **options)
        _, result = self.execute(wrong_owner)
        self.assertFalse(result["cleanup"])
        self.assertEqual(result["code"], "container_cleanup_unconfirmed")
        self.assertFalse(any(args[:2] == ["docker", "rm"] for args in self.calls))

    def test_cleanup_distinguishes_absence_from_daemon_failure(self):
        with patch.object(RUNNER.subprocess, "run", return_value=subprocess.CompletedProcess(
                [], 1, b"", b"Error: No such object: synthetic")):
            RUNNER.cleanup_container("synthetic", "label", "owner")
        with patch.object(RUNNER.subprocess, "run", return_value=subprocess.CompletedProcess(
                [], 1, b"", b"daemon unavailable")):
            with self.assertRaisesRegex(RuntimeError, "cleanup_unconfirmed"):
                RUNNER.cleanup_container("synthetic", "label", "owner")

    def test_timeouts_terminate_owned_process_group_and_clean_container(self):
        self.process.communicate.side_effect = [subprocess.TimeoutExpired("fixture", 120),
                                               subprocess.TimeoutExpired("fixture", 10), (b"", None)]
        with patch.object(RUNNER.os, "killpg") as kill:
            _, result = self.execute()
        self.assertEqual(kill.call_count, 2)
        self.assertTrue(result["cleanup"])
        self.assertEqual(result["status"], "BLOCKED")

    def test_remote_published_endpoint_is_refused_before_binary_start(self):
        def remote(arguments, **options):
            if arguments[:2] == ["docker", "port"]:
                return subprocess.CompletedProcess(arguments, 0, b"0.0.0.0:19000", b"")
            return self.external(arguments, **options)
        _, result = self.execute(remote)
        self.assertEqual(result["status"], "BLOCKED")
        self.process.communicate.assert_not_called()

    def test_existing_evidence_is_not_overwritten(self):
        self.execute()
        with self.assertRaisesRegex(RuntimeError, "existing_evidence_refused"):
            self.execute()


if __name__ == "__main__":
    unittest.main()
