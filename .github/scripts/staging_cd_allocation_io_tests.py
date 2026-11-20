"""Network budgets cannot become repeated or uncertain remote writes."""
import json
import subprocess
import unittest
from unittest.mock import Mock

from staging_cd_allocation_io import ALLOCATION_SECONDS, AllocationGitHub, AllocationIO, parallel_read
from staging_cd_allocation_snapshot import AllocationSnapshot
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
        io = AllocationIO(runner, clock=Mock(side_effect=[0, ALLOCATION_SECONDS + 1]))
        with self.assertRaisesRegex(Refusal, "deadline"):
            io.read(["fixture"])
        runner.assert_not_called()

    def test_push_after_slow_inventory_keeps_its_settlement_margin(self):
        # Hosted allocation reached the push after 23-28 s of inventory reads.
        runner = Mock(return_value=Mock(returncode=0, stdout=""))
        AllocationIO(runner, clock=Mock(side_effect=[0, 28])).write(["git", "push", "fixture"])
        self.assertEqual(runner.call_args.kwargs["timeout"], 10)
        runner.reset_mock()
        late = AllocationIO(runner, clock=Mock(side_effect=[0, ALLOCATION_SECONDS - 6.5]))
        with self.assertRaisesRegex(Refusal, "deadline"):
            late.write(["git", "push", "fixture"])
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

    def test_transport_and_visibility_share_three_physical_readbacks(self):
        name, oid = "cma-web/pending/v0.10.1", "a" * 40
        ok = response(200, json.dumps({"ref": "refs/tags/" + name, "object": {"type": "tag", "sha": oid}}))
        missing = response(404, '{"status":"404"}')
        for third in (ok, response(503)):
            runner = Mock(side_effect=[response(503), missing, third, ok])
            io = AllocationIO(runner, clock=lambda: 0, sleep=Mock())
            store = AllocationSnapshot(AllocationGitHub("fixture", io), "fixture", ".", io)
            if third is ok:
                store.verify(name, oid, "web")
            else:
                with self.assertRaisesRegex(Refusal, "unavailable"):
                    store.verify(name, oid, "web")
            self.assertEqual(runner.call_count, 3)
            self.assertTrue(all("GET" in call.args[0] for call in runner.call_args_list))
