"""Allocation refusals expose fixed codes without releasing private output."""
import contextlib
import io
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import Mock, patch

import staging_cd_allocate as allocation
from staging_cd_common import Refusal
import staging_cd_queue_tests
from staging_cd_reservations import next_reservation


class AllocationDiagnosticsTests(unittest.TestCase):
    def test_known_refusals_emit_only_fixed_codes(self):
        for message, code in allocation.REFUSAL_CODES.items():
            with self.subTest(code=code):
                output = io.StringIO()
                def operation(_masker):
                    print("private response")
                    raise Refusal(message)
                with contextlib.redirect_stdout(output):
                    status = allocation.run_allocation(operation)
                self.assertEqual(status, 1)
                self.assertEqual(output.getvalue(), "FAIL\nFAIL allocation " + code + "\n")

    def test_unknown_text_never_becomes_a_public_reason(self):
        for error, code in ((Refusal("private\n::warning::fixture"), "UNCLASSIFIED_REFUSAL"),
                            (ValueError("private fixture"), "UNEXPECTED_ERROR")):
            output = io.StringIO()
            def operation(_masker):
                raise error
            with contextlib.redirect_stdout(output):
                self.assertEqual(allocation.run_allocation(operation), 1)
            self.assertEqual(output.getvalue(), "FAIL\nFAIL allocation " + code + "\n")

    def test_success_preserves_pass_and_entrypoint_uses_boundary(self):
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            self.assertEqual(allocation.run_allocation(lambda _: True), 0)
        self.assertEqual(output.getvalue(), "PASS\n")
        with patch.object(allocation, "run_allocation", return_value=23) as boundary:
            self.assertEqual(allocation.main(), 23)
        boundary.assert_called_once()

    def test_unreadable_owner_refuses_before_new_reservations(self):
        fixture = staging_cd_queue_tests.QueueTests()
        queue, api, reservations = fixture.queue(), fixture.api(), Mock()
        change = queue["changes"][0]
        owned = next_reservation("web", change["source"], change["tree"], 1, "patch", 9, 1, [], [])
        reservations.list.return_value = [owned]
        api.request.side_effect = Refusal("GitHub API request failed")
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            status = allocation.run_allocation(lambda _: allocation.allocate_queue(queue, api, reservations, 10, 1))
        self.assertEqual(status, 1)
        self.assertEqual(output.getvalue(), "FAIL\nFAIL allocation RESERVATION_OWNER_UNREADABLE\n")
        reservations.insert.assert_not_called()
        api.request.assert_called_once_with("actions/runs/9/attempts/1", allow_not_found=True)

    def test_native_and_child_output_stays_hidden_with_reason(self):
        program = """
import os, subprocess, sys
from staging_cd_allocate import run_allocation
from staging_cd_common import Refusal
def operation(_masker):
    os.write(1, b'private native fixture')
    os.write(2, b'private native error')
    subprocess.run([sys.executable, '-c', "print('private child fixture')"], check=True)
    raise Refusal('Reservation owner run could not be read')
raise SystemExit(run_allocation(operation))
"""
        result = subprocess.run([sys.executable, "-B", "-c", program], cwd=Path(__file__).parent,
                                capture_output=True, text=True, check=False)
        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.stdout, "FAIL\nFAIL allocation RESERVATION_OWNER_UNREADABLE\n")
        self.assertEqual(result.stderr, "")


if __name__ == "__main__":
    unittest.main()
