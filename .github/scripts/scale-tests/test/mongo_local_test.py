"""Exercise fixture isolation/cleanup paths without contacting Docker."""
import importlib.util
import hashlib
import io
import json
from pathlib import Path
import subprocess
import tempfile
import sys
import unittest
from unittest.mock import Mock, patch


sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
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
        self.group = Mock()
        self.group.spawn.return_value = self.process
        self.group.cleanup.return_value = {"process_group_cleanup": True}

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
             patch.object(RUNNER, "OwnedGroup", return_value=self.group), \
             patch.object(RUNNER.secrets, "token_hex", side_effect=lambda n: "a" * (2 * n)), \
             patch("sys.stdout", new_callable=io.StringIO):
            code = RUNNER.run(self.binary, self.root / "result", "runner_binary::test")
        return code, json.loads((self.root / "result/mongo-process-results.json").read_text())

    def test_success_has_owned_cleanup_exact_count_and_no_credential_argv(self):
        self.process.communicate.return_value = (
            b"replica_fixture replicas=2 status=PASS ownership=true\n"
            b"test result: ok. 1 passed; 0 failed;", None)
        code, result = self.execute()
        self.assertEqual(code, 0)
        self.assertTrue(result["cleanup"])
        self.assertEqual(result["tests_passed"], 1)
        self.assertFalse(result["censored"])
        self.assertFalse(result["timed_out"])
        self.assertTrue(any(args[:2] == ["docker", "rm"] for args in self.calls))
        self.assertNotIn("a" * 48, json.dumps(self.calls))
        self.assertEqual(result["mongo_limits"]["db_tmpfs_mib"], 1024)
        self.assertEqual(result["completed_ownership_cohorts"], [{"replicas": 2, "enabled": True}])
        self.assertEqual(result["completed_replica_counts"], [2])

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
            b"fixture_diagnostic stage=index code=14031 kind=disk_space "
            b"ingress_rejection=routed_admission_unavailable "
            b"replica_failure replicas=2 ownership=true accepted=1 durable_plan=true target_present=1", None)
        _, result = self.execute()
        self.assertEqual(result["numeric_assertions"], [409, 202])
        self.assertEqual(result["migration_diagnostics"][0]["code"], 14031)
        self.assertEqual(result["failure_codes"], ["cross_ingress_not_accepted"])
        self.assertEqual(result["ingress_rejections"], ["routed_admission_unavailable"])
        self.assertTrue(result["failed_cohort_observations"][0]["durable_plan_after_responses"])
        self.assertNotIn("private-payload", json.dumps(result))

    def test_partial_container_creation_is_cleaned_after_run_fails(self):
        def fail_run(arguments, **options):
            if arguments[:2] == ["docker", "run"]:
                return subprocess.CompletedProcess(arguments, 1, b"", b"private-daemon-error")
            return self.external(arguments, **options)
        _, result = self.execute(fail_run)
        self.assertEqual(result["status"], "BLOCKED")
        self.assertEqual(result["failure_phase"], "container_create")
        self.assertEqual(result["error_class"], "fixture_error")
        self.assertEqual(result["code"], "local_fixture_command_failed")
        self.assertIsNone(result["output_sha256"])
        self.assertTrue(result["cleanup"])
        self.assertTrue(any(args[:2] == ["docker", "rm"] for args in self.calls))

    def test_cleanup_does_not_delete_an_unowned_container(self):
        def wrong_owner(arguments, **options):
            if arguments[:2] == ["docker", "inspect"]:
                return subprocess.CompletedProcess(arguments, 0, b"different-owner", b"")
            return self.external(arguments, **options)
        _, result = self.execute(wrong_owner)
        self.assertFalse(result["cleanup"])
        self.assertEqual(result["code"], "fixture_cleanup_unconfirmed")
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
        self.process.returncode = -9
        self.process.communicate.side_effect = [subprocess.TimeoutExpired("fixture", 120), (b"", None)]
        _, result = self.execute()
        self.group.cleanup.assert_called_once_with(self.process)
        self.assertTrue(result["cleanup"])
        self.assertEqual(result["status"], "BLOCKED")
        self.assertEqual(result["code"], "process_test_deadline")
        self.assertEqual(result["failure_phase"], "test_wait")
        self.assertEqual(result["termination_signal"], 9)
        self.assertTrue(result["censored"])
        self.assertTrue(result["timed_out"])

    def test_timeout_keeps_partial_whitelisted_output_and_never_claims_completion(self):
        partial = (b"private-body replica_fixture replicas=1 status=PASS ownership=true "
                   b"ingress_rejection=routed_admission_unavailable "
                   b"replica_failure replicas=2 ownership=true accepted=1 durable_plan=true target_present=0")
        self.process.returncode = -15
        self.process.communicate.side_effect = [subprocess.TimeoutExpired("fixture", 120, output=partial),
                                               (b"", None)]
        _, result = self.execute()
        self.assertEqual(result["output_sha256"], hashlib.sha256(partial).hexdigest())
        self.assertEqual(result["completed_ownership_cohorts"], [{"replicas": 1, "enabled": True}])
        self.assertEqual(result["failed_cohort_observations"][0]["accepted_responses"], 1)
        self.assertEqual(result["ingress_rejections"], ["routed_admission_unavailable"])
        self.assertEqual(result["exit_code"], -15)
        self.assertEqual(result["tests_passed"], 0)
        self.assertTrue(result["timed_out"])
        self.assertNotIn("private-body", json.dumps(result))

    def test_setup_timeout_is_separate_from_test_deadline(self):
        def setup_timeout(arguments, **options):
            if arguments[:2] == ["docker", "run"]:
                raise subprocess.TimeoutExpired("private-command", 60, output=b"private-output")
            return self.external(arguments, **options)
        _, result = self.execute(setup_timeout)
        self.assertEqual(result["failure_phase"], "container_create")
        self.assertEqual(result["error_class"], "command_timeout")
        self.assertTrue(result["timed_out"])
        self.assertTrue(result["cleanup"])
        self.assertIsNone(result["exit_code"])
        self.assertIsNone(result["output_sha256"])
        self.assertNotIn("private-", json.dumps(result))

    def test_unconfirmed_exit_retains_latest_partial_output_and_incomplete_cleanup(self):
        partial = b"replica_fixture replicas=1 status=PASS ownership=true"
        self.process.returncode = None
        self.process.communicate.side_effect = [
            subprocess.TimeoutExpired("fixture", 120),
            subprocess.TimeoutExpired("fixture", 2, output=partial)]
        self.group.cleanup.return_value = {"process_group_cleanup": False}
        _, result = self.execute()
        self.assertEqual(result["output_sha256"], hashlib.sha256(partial).hexdigest())
        self.assertFalse(result["process_cleanup"])
        self.assertFalse(result["container_cleanup"])
        self.assertTrue(result["fixture_directory_retained"])
        self.assertFalse(result["cleanup"])
        self.assertFalse(any(args[:2] == ["docker", "rm"] for args in self.calls))
        self.assertEqual(result["cleanup_failure_phase"], "process_termination")
        self.assertEqual(result["code"], "process_test_deadline")
        self.assertEqual(result["status"], "BLOCKED")

    def test_spawn_error_has_no_false_test_outcome(self):
        self.group.spawn.side_effect = OSError("private spawn detail")
        _, result = self.execute()
        self.group.cleanup.assert_called_once_with(None)
        self.assertEqual(result["failure_phase"], "test_spawn")
        self.assertEqual(result["error_class"], "os_error")
        self.assertTrue(result["cleanup"])
        self.assertNotIn("private spawn detail", json.dumps(result))

    def test_exited_parent_does_not_override_unconfirmed_group(self):
        self.group.cleanup.return_value = {"process_group_cleanup": False}
        _, result = self.execute()
        self.assertTrue(result["test_parent_cleanup"])
        self.assertFalse(result["process_group_cleanup"])
        self.assertFalse(result["cleanup"])
        self.assertEqual(result["status"], "BLOCKED")
        self.assertTrue(result["fixture_directory_retained"])
        self.assertTrue(list((self.root / "result").glob("process-fixture-*")))
        self.assertFalse(any(args[:2] == ["docker", "rm"] for args in self.calls))

    def test_unexpected_pipe_error_still_cleans_group_before_disposal(self):
        self.process.communicate.side_effect = [ValueError("private pipe failure"), (b"", None)]
        _, result = self.execute()
        self.group.cleanup.assert_called_once_with(self.process)
        self.assertTrue(result["cleanup"])
        self.assertFalse(result["fixture_directory_retained"])
        self.assertEqual(result["error_class"], "value_error")
        self.assertNotIn("private pipe failure", json.dumps(result))

    def test_mongo_readiness_deadline_has_setup_phase_and_no_test_output(self):
        def unready(arguments, **options):
            if arguments[:2] == ["docker", "exec"]:
                return subprocess.CompletedProcess(arguments, 1, b"private readiness", b"")
            return self.external(arguments, **options)
        with patch.object(RUNNER.time, "sleep"):
            _, result = self.execute(unready)
        self.assertEqual(result["failure_phase"], "mongo_initialize")
        self.assertEqual(result["code"], "mongo_readiness_deadline")
        self.assertTrue(result["timed_out"])
        self.assertTrue(result["cleanup"])
        self.assertIsNone(result["output_sha256"])
        self.process.communicate.assert_not_called()

    def test_remote_published_endpoint_is_refused_before_binary_start(self):
        def remote(arguments, **options):
            if arguments[:2] == ["docker", "port"]:
                return subprocess.CompletedProcess(arguments, 0, b"0.0.0.0:19000", b"")
            return self.external(arguments, **options)
        _, result = self.execute(remote)
        self.assertEqual(result["status"], "BLOCKED")
        self.process.communicate.assert_not_called()

    def test_timeout_retains_stage_start_without_end_or_success(self):
        partial = (
            b"replica_fixture replicas=3 status=PASS ownership=false\n"
            b"replica_stage replicas=3 ownership=false phase=teardown state=start ordinal=0 elapsed_ms=30000\n")
        self.process.returncode = -15
        self.process.communicate.side_effect = [subprocess.TimeoutExpired('fixture', 120, output=partial), (b'', None)]
        _, result = self.execute()
        self.assertEqual(result['status'], 'BLOCKED')
        self.assertTrue(result['censored'])
        self.assertEqual(result['tests_passed'], 0)
        self.assertEqual(result['cohort_stage_observations'], [{
            'replicas': 3, 'enabled': False, 'phase': 'teardown', 'boundary': 'start',
            'process_ordinal': 0, 'cohort_elapsed_ms': 30000}])
        self.assertEqual(result['ingress_header_observations'], [])

    def test_existing_evidence_is_not_overwritten(self):
        self.execute()
        with self.assertRaisesRegex(RuntimeError, "existing_evidence_refused"):
            self.execute()


if __name__ == "__main__":
    unittest.main()
