#!/usr/bin/env python3
"""Ownership regressions complement the real detached-Chrome watchdog probe."""
import signal
import subprocess
import sys
from pathlib import Path
import unittest
from unittest.mock import patch
from web_image_processes import OwnedProcesses, Process


class OwnershipTests(unittest.TestCase):
    def test_detached_and_reparented_descendants_remain_owned(self):
        table = {10: Process(10, 1, 10, "runner", "S"),
                 11: Process(11, 10, 10, "worker", "T"),
                 12: Process(12, 11, 12, "browser", "S"),
                 13: Process(13, 12, 12, "renderer", "S"),
                 20: Process(20, 1, 20, "unrelated", "S")}
        with patch("web_image_processes.snapshot", return_value=table):
            tree = OwnedProcesses(10)
            self.assertEqual(set(tree.capture()), {10, 11, 12, 13})
            table.pop(11)
            table[12] = Process(12, 1, 12, "browser", "S")
            self.assertEqual(set(tree.capture()), {10, 12, 13})
            self.assertNotIn(20, tree.owned)

    def test_adopted_unsampled_browser_is_owned_after_runner_is_reaped(self):
        table = {12: Process(12, 99, 12, "browser", "S"),
                 13: Process(13, 12, 12, "renderer", "S"),
                 20: Process(20, 1, 20, "unrelated", "S")}
        with patch("web_image_processes.snapshot", return_value=table):
            tree = OwnedProcesses(10, adopter=99)
            self.assertEqual(set(tree.capture()), {12, 13})
            self.assertNotIn(20, tree.owned)

    @unittest.skipUnless(sys.platform == "linux", "requires Linux subreaper")
    def test_real_adoption_without_any_snapshot_before_runner_reap(self):
        # A separate interpreter has no unrelated children. Deliberately postpone
        # the FIRST ownership snapshot until the runner has already been reaped.
        subprocess.run([sys.executable, "-c", """
import os, subprocess, sys
from web_image_processes import OwnedProcesses, child_subreaper
with child_subreaper():
    runner = subprocess.Popen([sys.executable, '-c',
        "import subprocess, sys; subprocess.Popen([sys.executable, '-c', "
        "'import time; time.sleep(60)'], start_new_session=True)"])
    assert runner.wait(timeout=5) == 0
    tree = OwnedProcesses(runner.pid, adopter=os.getpid())
    assert runner.pid not in tree.owned
    assert tree.capture(), 'unsampled adopted child was lost'
    tree.terminate()
    tree.reap()
    assert not tree.remaining
"""], cwd=Path(__file__).parent, check=True, timeout=15)

    def test_linux_signal_pins_identity_before_check_and_closes_handle(self):
        original = Process(10, 1, 10, "original", "S")
        with patch("web_image_processes.sys.platform", "linux"), \
                patch("web_image_processes.os.pidfd_open", create=True, return_value=42), \
                patch("web_image_processes.os.close") as close, \
                patch("web_image_processes.signal.pidfd_send_signal", create=True) as send, \
                patch("web_image_processes.snapshot", return_value={10: original}) as table:
            self.assertTrue(OwnedProcesses.send(original, signal.SIGKILL))
            send.assert_called_once_with(42, signal.SIGKILL)
            table.return_value = {10: Process(10, 1, 10, "reused", "S")}
            self.assertFalse(OwnedProcesses.send(original, signal.SIGKILL))
            self.assertEqual(send.call_count, 1)
            self.assertEqual(close.call_count, 2)

    def test_reused_pid_and_its_children_are_never_signalled(self):
        original = Process(10, 1, 10, "original", "S")
        table = {10: original}
        with patch("web_image_processes.sys.platform", "darwin"), \
                patch("web_image_processes.snapshot", return_value=table), \
                patch("web_image_processes.os.kill") as kill, \
                patch("web_image_processes.os.waitpid") as wait:
            tree = OwnedProcesses(10)
            table[10] = Process(10, 1, 10, "replacement", "S")
            table[11] = Process(11, 10, 10, "replacement-child", "S")
            self.assertEqual(tree.capture(), {})
            self.assertFalse(tree.send(original, signal.SIGKILL))
            tree.reap()
            kill.assert_not_called()
            wait.assert_not_called()

    def test_exited_process_is_not_an_error_but_permissions_are(self):
        process = Process(10, 1, 10, "runner", "S")
        with patch("web_image_processes.sys.platform", "darwin"), \
                patch("web_image_processes.snapshot", return_value={10: process}), \
                patch("web_image_processes.os.kill", side_effect=ProcessLookupError):
            self.assertFalse(OwnedProcesses.send(process, signal.SIGKILL))
        with patch("web_image_processes.sys.platform", "darwin"), \
                patch("web_image_processes.snapshot", return_value={10: process}), \
                patch("web_image_processes.os.kill", side_effect=PermissionError):
            with self.assertRaises(PermissionError):
                OwnedProcesses.send(process, signal.SIGKILL)


if __name__ == "__main__":
    unittest.main()
