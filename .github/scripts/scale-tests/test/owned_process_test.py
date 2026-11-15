"""Real local custody regressions: no Docker, database, gateway or cluster."""
import os
import hashlib
import io
import json
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch
from mongo_local_test import RUNNER

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from owned_process import CustodyError, OwnedGroup, group_members


# The descendant closes inherited pipes, ignores TERM and keeps a file open.
# The parent has default TERM behavior, or exits immediately for the exit case.
FAMILY = r'''
import os, signal, sys, time
pid = os.fork()
if pid == 0:
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
    handle = open(sys.argv[1], "w")
    os.dup2(handle.fileno(), 1)
    os.dup2(handle.fileno(), 2)
    print(os.getpid(), flush=True)
    time.sleep(60)
    os._exit(0)
while not os.path.exists(sys.argv[1]) or os.stat(sys.argv[1]).st_size == 0:
    time.sleep(0.01)
if sys.argv[2] == "exit":
    sys.exit(3)
time.sleep(60)
'''


class OwnedGroupTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(dir=Path(__file__).parent)
        self.addCleanup(self.directory.cleanup)
        self.group = OwnedGroup()
        self.group.start()
        self.process = None
        self.addCleanup(self.dispose)

    def dispose(self):
        receipt = self.group.cleanup(self.process, grace=0.1, kill_wait=3)
        self.assertTrue(receipt["process_group_cleanup"], "local regression cleanup incomplete")
        if self.process is not None:
            self.process.communicate(timeout=2)

    def spawn_family(self, mode):
        path = Path(self.directory.name) / "descendant.log"
        self.process = self.group.spawn([sys.executable, "-c", FAMILY, str(path), mode],
                                       stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        deadline = time.monotonic() + 3
        while not path.exists() or not path.read_text().strip():
            if time.monotonic() >= deadline:
                self.fail("local descendant did not start")
            time.sleep(0.01)
        return int(path.read_text().strip())

    def test_term_exited_parent_with_file_backed_descendant_is_killed_and_observed(self):
        descendant = self.spawn_family("wait")
        self.group.signal(signal.SIGTERM)
        self.process.communicate(timeout=2)
        self.assertEqual(self.process.returncode, -signal.SIGTERM)
        self.assertIn(descendant, group_members(self.group.anchor.pid))
        result = self.group.cleanup(self.process, grace=0.1, kill_wait=3)
        self.assertTrue(result["group_kill_sent"])
        self.assertTrue(result["process_group_cleanup"])
        self.assertNotIn(descendant, group_members(self.group.anchor.pid))

    def test_early_failed_parent_still_requires_descendant_cleanup(self):
        descendant = self.spawn_family("exit")
        self.process.communicate(timeout=2)
        self.assertEqual(self.process.returncode, 3)
        self.assertIn(descendant, group_members(self.group.anchor.pid))
        result = self.group.cleanup(self.process, grace=0.1, kill_wait=3)
        self.assertTrue(result["group_kill_sent"])
        self.assertTrue(result["process_group_cleanup"])

    def test_normal_completion_and_already_absent_group_are_idempotent(self):
        self.process = self.group.spawn([sys.executable, "-c", "pass"], stdout=subprocess.PIPE)
        self.process.communicate(timeout=2)
        result = self.group.cleanup(self.process, grace=0.1, kill_wait=3)
        self.assertTrue(result["process_group_cleanup"])
        self.assertFalse(result["group_term_sent"])
        with patch("owned_process.os.killpg") as send:
            self.assertTrue(self.group.cleanup()["process_group_cleanup"])
        send.assert_not_called()

    def test_failed_snapshot_attempts_termination_but_cannot_claim_absence(self):
        self.spawn_family("exit")
        self.process.communicate(timeout=2)
        with patch("owned_process.group_members", side_effect=OSError("private detail")):
            result = self.group.cleanup(self.process, grace=0.1, kill_wait=3)
        self.assertFalse(result["process_group_cleanup"])
        self.assertTrue(result["group_term_sent"])
        self.assertTrue(result["group_kill_sent"])
        self.assertTrue(result["group_observation_failed"])
        self.assertTrue(self.group.reserved())  # Killed anchor remains unreaped.

    def test_lost_reservation_never_signals_a_reused_group(self):
        with patch.object(self.group, "reserved", return_value=False), \
             patch("owned_process.os.killpg") as send:
            result = self.group.cleanup(grace=0.1, kill_wait=3)
        self.assertFalse(result["process_group_cleanup"])
        send.assert_not_called()

    def test_snapshot_rejects_failed_and_invalid_numeric_output(self):
        for value in [subprocess.CompletedProcess([], 1, b"", b"private"),
                      subprocess.CompletedProcess([], 0, b"1 2 3", b"")]:
            with patch("owned_process.subprocess.run", return_value=value):
                with self.assertRaises(CustodyError):
                    group_members(1)

    def test_auto_reaping_or_threaded_callers_are_refused_before_spawn(self):
        for target, value in [("owned_process.threading.active_count", 2),
                              ("owned_process.signal.getsignal", signal.SIG_IGN)]:
            with patch(target, return_value=value), patch("owned_process.subprocess.Popen") as start:
                with self.assertRaises(CustodyError):
                    OwnedGroup().start()
                start.assert_not_called()

    def test_runner_early_failure_disposes_only_after_real_descendant_is_gone(self):
        binary = Path(self.directory.name) / "fixture-parent"
        source = FAMILY.replace('sys.argv[1]', 'os.path.join(os.environ["TMPDIR"], "descendant.log")')
        source = source.replace('sys.argv[2]', '"exit"')
        binary.write_text("#!" + sys.executable + "\n" + source)
        binary.chmod(0o700)
        actual_run = subprocess.run
        # This test already owns a ready anchor; let the complete runner reuse
        # it. All Docker operations are mocked; ps and the local family are real.
        removed = []

        def external(arguments, **options):
            if arguments[0] == "/bin/ps":
                return actual_run(arguments, **options)
            if arguments[:2] == ["docker", "inspect"]:
                return subprocess.CompletedProcess(arguments, 0, b"a" * 24, b"")
            if arguments[:2] == ["docker", "port"]:
                return subprocess.CompletedProcess(arguments, 0, b"127.0.0.1:19000", b"")
            if arguments[:2] == ["docker", "rm"]:
                self.assertTrue(self.group.confirmed)
                self.assertFalse(group_members(self.group.anchor.pid))
                removed.append(True)
            return subprocess.CompletedProcess(arguments, 0, b"", b"")

        with patch.object(RUNNER, "OwnedGroup", return_value=self.group), \
             patch.object(self.group, "start"), \
             patch.object(RUNNER.subprocess, "run", side_effect=external), \
             patch.object(RUNNER.secrets, "token_hex", side_effect=lambda n: "a" * (2 * n)), \
             patch("sys.stdout", new_callable=io.StringIO):
            code = RUNNER.run(binary, Path(self.directory.name) / "result", "runner_binary::test")
        result = json.loads((Path(self.directory.name) / "result/mongo-process-results.json").read_text())
        self.assertEqual(code, 2)
        self.assertEqual(result["status"], "FAIL")
        self.assertEqual(result["exit_code"], 3)
        self.assertTrue(result["test_parent_cleanup"])
        self.assertTrue(result["process_group_cleanup"])
        self.assertTrue(result["group_kill_sent"])
        self.assertTrue(result["cleanup"])
        self.assertFalse(result["fixture_directory_retained"])
        self.assertEqual(result["output_sha256"], hashlib.sha256(b"").hexdigest())
        self.assertEqual(removed, [True])


if __name__ == "__main__":
    unittest.main()
