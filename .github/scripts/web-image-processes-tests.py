#!/usr/bin/env python3
"""Ownership regressions complement the real detached-Chrome watchdog probe."""
import signal
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

    def test_reused_pid_and_its_children_are_never_signalled(self):
        original = Process(10, 1, 10, "original", "S")
        table = {10: original}
        with patch("web_image_processes.snapshot", return_value=table), \
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
        with patch("web_image_processes.snapshot", return_value={10: process}), \
                patch("web_image_processes.os.kill", side_effect=ProcessLookupError):
            self.assertFalse(OwnedProcesses.send(process, signal.SIGKILL))
        with patch("web_image_processes.snapshot", return_value={10: process}), \
                patch("web_image_processes.os.kill", side_effect=PermissionError):
            with self.assertRaises(PermissionError):
                OwnedProcesses.send(process, signal.SIGKILL)


if __name__ == "__main__":
    unittest.main()
