"""Allocation refusals expose fixed codes without releasing private output."""
import contextlib
import io
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import Mock, patch

import staging_cd_allocate as allocation
from staging_cd_allocation_io import AllocationGitHub, AllocationIO
from staging_cd_common import Refusal
from staging_cd_github import GitHub
import staging_cd_queue_tests
from staging_cd_reservations import next_reservation
from staging_cd_outcome_tests import legacy


class AllocationDiagnosticsTests(unittest.TestCase):
    def allocate_privately(self, operation):
        """Run allocation; return its status, its public log and the step outputs it wrote."""
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "output"
            path.touch()
            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                status = allocation.run_allocation(operation, {"GITHUB_OUTPUT": str(path)})
            return status, legacy(output.getvalue()), path.read_text()

    def test_known_refusals_emit_only_fixed_codes(self):
        for message, code in allocation.REFUSAL_CODES.items():
            with self.subTest(code=code):
                def operation(_masker):
                    print("private response")
                    raise Refusal(message)
                status, log, outputs = self.allocate_privately(operation)
                if code in allocation.TRANSIENT_CODES:
                    # The step records the code and succeeds; its settlement gate reports it.
                    self.assertEqual((status, log), (0, "FAIL allocation\n"))
                    self.assertRegex(outputs, rf"\Atransient=true\nrefused={code}\n"
                                              r"site=staging_cd_allocate_diagnostics_tests\.py:\d+\n\Z")
                else:
                    self.assertEqual((status, log, outputs), (1, "FAIL allocation\nFAIL allocation " + code + "\n", ""))

    def test_a_failed_github_read_is_named_by_its_cause(self):
        def http(status, headers=""):
            return subprocess.CompletedProcess([], 1, f"HTTP/2.0 {status} Status\n{headers}\n{{}}", "")

        def refused(result):
            with self.assertRaisesRegex(Refusal, "GitHub API request failed") as raised:
                GitHub.response("pulls/1", None, result, False)
            return raised.exception
        reset = "Get \"https://api.github.com/repos\": read tcp: connection reset by peer"
        for result, code in (
                (http(500), "GITHUB_READ_TRANSIENT"), (http(503), "GITHUB_READ_TRANSIENT"),
                (http(429), "GITHUB_READ_TRANSIENT"),
                (http(403, "X-RateLimit-Remaining: 0\n"), "GITHUB_READ_TRANSIENT"),
                (http(403, "Retry-After: 60\n"), "GITHUB_READ_TRANSIENT"),
                # No response at all, and a diagnostic that names the network.
                (subprocess.CompletedProcess([], 4, "", reset), "GITHUB_READ_TRANSIENT"),
                (http(401), "GITHUB_READ_DENIED"),
                (http(403, "X-RateLimit-Remaining: 4999\n"), "GITHUB_READ_DENIED"),
                (http(422), "GITHUB_READ_FAILED"),
                # Neither a network fault nor a denial is proven.
                (subprocess.CompletedProcess([], 4, "", "gh auth login is required"), "GITHUB_READ_FAILED"),
                (subprocess.CompletedProcess([], 0, "HTTP/2.0 200 OK\n\n" + "x" * (16 * 1024 * 1024), ""),
                 "GITHUB_READ_FAILED")):
            with self.subTest(code=code, result=result.stdout[:40] + result.stderr[:40]):
                error = refused(result)
                self.assertEqual(allocation.refusal_code(error), code)
                status, log, outputs = self.allocate_privately(lambda _masker, error=error: (_ for _ in ()).throw(error))
                if code in allocation.TRANSIENT_CODES:
                    self.assertEqual((status, log), (0, "FAIL allocation\n"))
                    self.assertIn(f"refused={code}\n", outputs)
                else:
                    self.assertEqual((status, log, outputs), (1, f"FAIL allocation\nFAIL allocation {code}\n", ""))
        self.assertLessEqual(allocation.GITHUB_READ_CAUSES, allocation.TRANSIENT_CODES | {"GITHUB_READ_DENIED"})

    def test_allocation_reads_keep_their_cause_after_repeats_and_a_spent_budget(self):
        throttled = subprocess.CompletedProcess([], 1, "HTTP/2.0 403 Status\nRetry-After: 1\n\n{}", "")
        for result, code, calls in ((throttled, "GITHUB_READ_TRANSIENT", 3),
                                    (subprocess.CompletedProcess([], 1, "HTTP/2.0 403 Status\n\n{}", ""),
                                     "GITHUB_READ_DENIED", 1),
                                    # A 5xx is already a repeated allocation read failure.
                                    (subprocess.CompletedProcess([], 1, "HTTP/2.0 502 Status\n\n{}", ""),
                                     "READ_FAILED", 3)):
            runner = Mock(return_value=result)
            io = AllocationIO(runner, clock=lambda: 0, sleep=Mock(), rand=lambda: 0.0)
            with self.subTest(code=code), self.assertRaises(Refusal) as raised:
                AllocationGitHub("fixture", io).request("pulls/1")
            self.assertEqual((allocation.refusal_code(raised.exception), runner.call_count), (code, calls))
        # A spent read budget refused nothing; it used to be an unclassified refusal.
        with self.assertRaisesRegex(Refusal, "budget expired") as raised:
            GitHub("fixture", Mock(), retry={"clock": lambda: 100}, deadline=100).request("pulls/1")
        self.assertEqual(allocation.refusal_code(raised.exception), "DEADLINE_EXCEEDED")
        self.assertIn("DEADLINE_EXCEEDED", allocation.TRANSIENT_CODES)

    def test_unknown_text_never_becomes_a_public_reason(self):
        for error, code in ((Refusal("private\n::warning::fixture"), "UNCLASSIFIED_REFUSAL"),
                            (ValueError("private fixture"), "UNEXPECTED_ERROR")):
            output = io.StringIO()
            def operation(_masker):
                raise error
            with contextlib.redirect_stdout(output):
                self.assertEqual(allocation.run_allocation(operation), 1)
            self.assertEqual(legacy(output.getvalue()), "FAIL allocation\nFAIL allocation " + code + "\n")

    def test_success_preserves_pass_and_entrypoint_uses_boundary(self):
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            self.assertEqual(allocation.run_allocation(lambda _: True), 0)
        self.assertEqual(output.getvalue(), "PASS allocation\n")
        with patch.object(allocation, "run_allocation", return_value=23) as boundary:
            self.assertEqual(allocation.main(), 23)
        boundary.assert_called_once()
        self.assertIs(boundary.call_args.args[1], allocation.os.environ)

    def test_unreadable_owner_refuses_before_new_reservations(self):
        fixture = staging_cd_queue_tests.QueueTests()
        queue, api, reservations = fixture.queue(), fixture.api(), Mock()
        change = queue["changes"][-1]
        owned = next_reservation("web", change["source"], change["tree"], 1, "patch", 9, 1, [], [])
        reservations.list.return_value = [owned]
        api.request.side_effect = Refusal("GitHub API request failed")
        status, log, outputs = self.allocate_privately(
            lambda _: allocation.allocate_queue(queue, api, reservations, 10, 1))
        # An unreadable owner is transient: the settlement gate fails the job.
        self.assertEqual((status, log), (0, "FAIL allocation\n"))
        self.assertRegex(outputs, r"\Atransient=true\nrefused=RESERVATION_OWNER_UNREADABLE\nsite=staging_cd_allocate\.py:\d+\n\Z")
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
    raise Refusal('Reservation run identity is invalid')
raise SystemExit(run_allocation(operation))
"""
        result = subprocess.run([sys.executable, "-B", "-c", program], cwd=Path(__file__).parent,
                                capture_output=True, text=True, check=False)
        self.assertEqual(result.returncode, 1)
        self.assertEqual(legacy(result.stdout), "FAIL allocation\nFAIL allocation RESERVATION_OWNER_INVALID\n")
        self.assertEqual(result.stderr, "")


if __name__ == "__main__":
    unittest.main()
