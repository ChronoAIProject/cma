"""Network budgets cannot become repeated or uncertain remote writes."""
import json
import subprocess
import threading
import time
import unittest
from unittest.mock import Mock, patch

from staging_cd_allocation_io import (ALLOCATION_SECONDS, PUSH_RESERVE_SECONDS, PUSH_SECONDS, PUSH_SETTLEMENT_SECONDS, AllocationGitHub, AllocationIO,
                                      parallel_read)
from staging_cd_allocation_snapshot import AllocationSnapshot
from staging_cd_common import Refusal
from staging_cd_github import GitHub, GitHubNotFound
import staging_cd_github
import staging_cd_allocation_io


def response(code, body="{}"):
    return subprocess.CompletedProcess([], 0 if code == 200 else 1, f"HTTP/2.0 {code} Status\n\n{body}", "")


class AllocationIOTests(unittest.TestCase):
    def test_parallel_first_history_reads_share_one_fetch(self):
        # Annotation reads run on several threads. Concurrent fetches into one
        # checkout race on its lock files, so only one load may ever run.
        for module, client in ((staging_cd_github, lambda: GitHub("fixture")),
                               (staging_cd_allocation_io, lambda: AllocationGitHub("fixture", AllocationIO(Mock())))):
            active, peak, loads, lock = [0], [0], [], threading.Lock()

            def load(*_args, **_kwargs):
                with lock:
                    active[0] += 1
                    peak[0] = max(peak[0], active[0])
                time.sleep(0.05)
                with lock:
                    active[0] -= 1
                loads.append(1)
                return object()

            api = client()
            with self.subTest(client=type(api).__name__), patch.object(module.DevelopHistory, "load", side_effect=load):
                results = parallel_read(lambda _: api.history(), range(4))
                self.assertEqual((len(loads), peak[0]), (1, 1))
                self.assertTrue(all(result is results[0] for result in results))

    def test_transient_reads_back_off_with_jitter_inside_the_allocation_deadline(self):
        for failure in (response(429), response(503), subprocess.TimeoutExpired("fixture", 5)):
            runner = Mock(side_effect=[failure, failure, response(200)])
            io = AllocationIO(runner, clock=lambda: 0, sleep=Mock(), rand=lambda: 1.0)
            self.assertEqual(AllocationGitHub("fixture", io).request("pulls/1"), {})
            self.assertEqual(runner.call_count, 3)
            self.assertEqual([call.kwargs["timeout"] for call in runner.call_args_list], [5, 5, 5])
            self.assertEqual([call.args[0] for call in io.sleep.call_args_list], [0.5, 1.0])
        # A repeat that would reach the push reserve is never started, so a slow
        # read fails while the reservation push can still run.
        for now, repeats in ((ALLOCATION_SECONDS - PUSH_RESERVE_SECONDS - 0.6, True),
                             (ALLOCATION_SECONDS - PUSH_RESERVE_SECONDS - 0.4, False),
                             (ALLOCATION_SECONDS - 0.4, False)):
            with self.subTest(now=now):
                runner = Mock(side_effect=[response(503), response(200)])
                late = AllocationIO(runner, clock=Mock(side_effect=[0, *[now] * 4]), sleep=Mock(), rand=lambda: 1.0)
                if repeats:
                    self.assertEqual(AllocationGitHub("fixture", late).request("pulls/1"), {})
                    self.assertEqual(runner.call_count, 2)
                    continue
                with self.assertRaisesRegex(Refusal, "Allocation read failed"):
                    AllocationGitHub("fixture", late).request("pulls/1")
                runner.assert_called_once()
                late.sleep.assert_not_called()
        # The reserve is exactly what a push still requires when it starts.
        runner = Mock(return_value=Mock(returncode=0, stdout=""))
        AllocationIO(runner, clock=Mock(side_effect=[0, ALLOCATION_SECONDS - PUSH_RESERVE_SECONDS])).write(
            ["git", "push", "fixture"])
        runner.assert_called_once()

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
        # GitHub can take longer than 10 s to accept the atomic push, so it gets
        # PUSH_SECONDS while the settlement margin stays reserved after it.
        runner = Mock(return_value=Mock(returncode=0, stdout=""))
        AllocationIO(runner, clock=Mock(side_effect=[0, 28])).write(["git", "push", "fixture"])
        self.assertEqual(runner.call_args.kwargs["timeout"], PUSH_SECONDS)
        runner.reset_mock()
        # Close to the deadline the push is bounded by what leaves settlement time.
        AllocationIO(runner, clock=Mock(side_effect=[0, ALLOCATION_SECONDS - PUSH_SETTLEMENT_SECONDS - 12])).write(
            ["git", "push", "fixture"])
        self.assertEqual(runner.call_args.kwargs["timeout"], 12)
        runner.reset_mock()
        late = AllocationIO(runner, clock=Mock(side_effect=[0, ALLOCATION_SECONDS - PUSH_RESERVE_SECONDS + 0.5]))
        with self.assertRaisesRegex(Refusal, "deadline"):
            late.write(["git", "push", "fixture"])
        runner.assert_not_called()

    def test_retries_exhaust_and_uncertain_write_is_never_repeated(self):
        runner = Mock(side_effect=subprocess.TimeoutExpired("fixture", 5))
        io = AllocationIO(runner, clock=lambda: 0, sleep=Mock())
        with self.assertRaises(Refusal):
            io.read(["fixture"])
        self.assertEqual(runner.call_count, 3)
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
