"""Network budgets cannot become repeated or uncertain remote writes."""
import subprocess
import unittest
from unittest.mock import Mock

from staging_cd_allocation_io import AllocationGitHub, AllocationIO, parallel_read
from staging_cd_common import Refusal
from staging_cd_github import GitHubNotFound


def response(code, body="{}"):
    return subprocess.CompletedProcess([], 0 if code == 200 else 1, f"HTTP/2.0 {code} Status\n\n{body}", "")


class AllocationIOTests(unittest.TestCase):
    def test_transient_gets_retry_once_with_short_deadline(self):
        for failure in (response(429), response(503), subprocess.TimeoutExpired("fixture", 5)):
            runner = Mock(side_effect=[failure, response(200)])
            io = AllocationIO(runner, clock=lambda: 0, sleep=Mock())
            self.assertEqual(AllocationGitHub("fixture", io).request("pulls/1"), {})
            self.assertEqual(runner.call_count, 2)
            self.assertEqual([call.kwargs["timeout"] for call in runner.call_args_list], [5, 5])
            io.sleep.assert_called_once_with(0.2)

    def test_auth_and_missing_provenance_do_not_retry(self):
        for code in (401, 403, 404):
            runner = Mock(return_value=response(code, '{"status":"404"}'))
            with self.assertRaises(Refusal):
                AllocationGitHub("fixture", AllocationIO(runner)).request("pulls/1")
            runner.assert_called_once()
        runner = Mock(return_value=response(404, '{"status":"404"}'))
        with self.assertRaises(GitHubNotFound):
            AllocationGitHub("fixture", AllocationIO(runner)).request("pulls/1", allow_not_found=True)

    def test_deadline_refuses_without_starting_subprocess(self):
        runner = Mock()
        io = AllocationIO(runner, clock=Mock(side_effect=[0, 31]))
        with self.assertRaisesRegex(Refusal, "deadline"):
            io.read(["fixture"])
        runner.assert_not_called()

    def test_retries_exhaust_and_uncertain_write_is_never_repeated(self):
        runner = Mock(side_effect=subprocess.TimeoutExpired("fixture", 5))
        io = AllocationIO(runner, clock=lambda: 0, sleep=Mock())
        with self.assertRaises(Refusal):
            io.read(["fixture"])
        self.assertEqual(runner.call_count, 2)
        runner.reset_mock()
        with self.assertRaisesRegex(Refusal, "settlement"):
            io.write(["fixture"])
        runner.assert_called_once()
        self.assertEqual(runner.call_args.kwargs["timeout"], 10)

    def test_api_writer_is_refused_and_pool_propagates_failure(self):
        runner = Mock()
        api = AllocationGitHub("fixture", AllocationIO(runner))
        with self.assertRaises(Refusal):
            api.request("git/refs", {"ref": "fixture"})
        runner.assert_not_called()
        self.assertEqual(parallel_read(lambda n: n + 1, []), [])
        with self.assertRaises(ValueError):
            parallel_read(lambda n: int(n), ["1", "invalid"])
