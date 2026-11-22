"""Allocation progress is live while arbitrary output stays suppressed."""
import contextlib
import io
import json
import os
import selectors
import subprocess
import sys
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

import staging_cd_allocate as allocation
import staging_cd_queue_tests as fixtures
from staging_cd_allocate_progress import allocation_progress
from staging_cd_common import Refusal


class AllocationProgressTests(unittest.TestCase):
    def test_only_closed_categories_and_elapsed_seconds_can_escape(self):
        output = io.StringIO()
        with contextlib.redirect_stdout(output), allocation_progress(Mock(side_effect=[10, 12.9])) as mark:
            mark("am", "owner-read")
            for component, phase in (("private-value", "reserve"), ("am", "private\n::warning::payload")):
                with self.assertRaises(Refusal):
                    mark(component, phase)
        self.assertEqual(output.getvalue(),
                         "PROGRESS allocation component=am phase=owner-read count=1 elapsed_seconds=2\n")
        for value in (float("nan"), float("inf"), -1):
            output = io.StringIO()
            with contextlib.redirect_stdout(output), allocation_progress(Mock(side_effect=[0, value])) as mark:
                with self.assertRaises(Refusal):
                    mark("all", "history")
            self.assertEqual(output.getvalue(), "")

    def test_a_rerun_prints_one_fixed_retry_line_and_nothing_else(self):
        output = io.StringIO()
        with contextlib.redirect_stdout(output), allocation_progress(lambda: 0) as mark:
            mark.retry("READ_FAILED", 2, 2, 14.0)
            for args in (("private\n::warning::payload", 2, 2, 1.0), ("READ_FAILED", 1, 2, 1.0),
                         ("READ_FAILED", 3, 2, 1.0), ("READ_FAILED", True, 2, 1.0),
                         ("READ_FAILED", 2, 2, float("nan")), ("READ_FAILED", 2, 2, -1)):
                with self.subTest(args=args), self.assertRaises(Refusal):
                    mark.retry(*args)
        self.assertEqual(output.getvalue(), "RETRY allocation reason=READ_FAILED attempt=2/2 wait=14.0s\n")

    def test_progress_arrives_before_completion_without_native_or_child_output(self):
        program = """
import os, subprocess, sys
from staging_cd_allocate import run_allocation
from staging_cd_allocate_progress import allocation_progress
with allocation_progress(lambda: 0) as mark:
    def operation(_masker):
        os.write(1, b'private native output')
        os.write(2, b'private native error')
        subprocess.run([sys.executable, '-c', "print('private child output')"], check=True)
        mark('am', 'owner-read')
        sys.stdin.readline()
        mark('all', 'complete')
        return True
    raise SystemExit(run_allocation(operation))
"""
        with subprocess.Popen([sys.executable, "-B", "-c", program], cwd=Path(__file__).parent,
                              stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                              text=True) as child:
            try:
                with selectors.DefaultSelector() as selector:
                    selector.register(child.stdout, selectors.EVENT_READ)
                    self.assertTrue(selector.select(timeout=10), "Progress was withheld until completion")
                first = child.stdout.readline()
                self.assertIsNone(child.poll())
                stdout, stderr = child.communicate("continue\n", timeout=10)
            finally:
                if child.poll() is None:
                    child.kill()
                    child.communicate()
            self.assertEqual(child.returncode, 0)
        self.assertEqual(first, "PROGRESS allocation component=am phase=owner-read count=1 elapsed_seconds=0\n")
        self.assertEqual(stdout, "PROGRESS allocation component=all phase=complete count=1 elapsed_seconds=0\nPASS allocation\n")
        self.assertEqual(stderr, "")

    def test_allocator_reports_each_io_phase_and_keeps_newest_only(self):
        fixture = fixtures.QueueTests()
        pending, marks = [], []
        reservations = Mock()
        reservations.list.side_effect = lambda: list(pending)
        reservations.insert.side_effect = lambda value: pending.append(value) is None
        result = allocation.allocate_queue(fixture.queue(), fixture.api(), reservations, 10, 1,
                                           mark=lambda *args: marks.append(args))
        self.assertEqual(marks[:3], [("all", "final-inventory"), ("all", "pending-inventory"), ("all", "history")])
        self.assertEqual(marks[-1], ("all", "planned"))
        self.assertEqual([phase for component, phase in marks if component == "web"],
                         ["merged-pr", "reserve"])
        self.assertEqual(len(json.loads(result["include"][0]["tasks"])), 1)
        api = fixture.api()
        api.request.side_effect = Refusal("private owner response")
        with self.assertRaises(Refusal):
            allocation.allocate_queue(fixture.queue(), api, reservations, 11, 1,
                                      mark=lambda *args: marks.append(args))
        self.assertEqual(marks[-1], ("web", "owner-read"))

    def test_entrypoint_passes_live_reporter_through_private_boundary(self):
        output = io.StringIO()
        env = {"GITHUB_EVENT_NAME": "push", "GITHUB_RUN_ID": "1", "GITHUB_RUN_ATTEMPT": "1", "GITHUB_SHA": "a" * 40,
               "CD_QUEUE": json.dumps({"event_source": "a" * 40})}
        def allocate(_queue, _env, _io, mark):
            mark("web", "reserve")
            return {"include": []}
        with patch.dict(os.environ, env, clear=True), contextlib.redirect_stdout(output), \
                patch.object(allocation, "event_identity"), patch.object(allocation, "GitHub"), \
                patch.object(allocation, "Reservations"), patch.object(allocation, "output"), \
                patch.object(allocation, "allocate_current", side_effect=allocate):
            self.assertEqual(allocation.main(), 0)
        self.assertRegex(output.getvalue(),
                         r"^PROGRESS allocation component=web phase=reserve count=1 elapsed_seconds=\d+\n"
                         r"PROGRESS allocation component=all phase=complete count=1 elapsed_seconds=\d+\nPASS allocation\n$")


if __name__ == "__main__":
    unittest.main()
