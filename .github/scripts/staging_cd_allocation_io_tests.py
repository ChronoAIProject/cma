"""Network budgets cannot become repeated or uncertain remote writes."""
import json
import subprocess
import threading
import time
import unittest
from unittest.mock import Mock, patch

from staging_cd_allocation_io import (ALLOCATION_SECONDS, PUSH_RESERVE_SECONDS, PUSH_SECONDS, PUSH_SETTLEMENT_SECONDS,
                                      SETTLE_WAITS, VERIFY_RESERVE_SECONDS, AllocationGitHub, AllocationIO,
                                      TransientRead, UncertainPush, parallel_read)
from staging_cd_allocation_snapshot import AllocationSnapshot
from staging_cd_common import Refusal
from staging_cd_github import GitHub, GitHubNotFound
import ci_retry
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

    def test_git_fatal_outage_is_repeated_but_other_fatal_errors_are_returned(self):
        # Git exits 128 for every fatal error, so only its diagnostic marks an outage.
        outage = subprocess.CompletedProcess([], 128, "", "fatal: unable to access "
                                             "'https://github.com/ChronoAIProject/cma.git/': "
                                             "The requested URL returned error: 502\n")
        refused = subprocess.CompletedProcess([], 128, "", "fatal: repository "
                                              "'https://github.com/ChronoAIProject/cma.git/' not found\n")
        runner = Mock(side_effect=[outage, outage, refused])
        io = AllocationIO(runner, clock=lambda: 0, sleep=Mock(), rand=lambda: 1.0)
        self.assertIs(io.read(["git", "ls-remote"]), refused)
        self.assertEqual(runner.call_count, 3)
        self.assertEqual([call.args[0] for call in io.sleep.call_args_list], [0.5, 1.0])
        runner = Mock(return_value=outage)
        with self.assertRaisesRegex(Refusal, "Allocation read failed") as raised:
            AllocationIO(runner, clock=lambda: 0, sleep=Mock()).read(["git", "ls-remote"])
        self.assertIsInstance(raised.exception, TransientRead)
        self.assertEqual(runner.call_count, 3)
        # Only Git reports a fatal error this way; another client's exit 128 is returned.
        runner = Mock(return_value=outage)
        self.assertIs(AllocationIO(runner, clock=lambda: 0, sleep=Mock()).read(["gh", "api"]), outage)
        runner.assert_called_once()

    def test_a_failed_read_keeps_the_wait_a_throttled_response_asked_for(self):
        throttled = subprocess.CompletedProcess([], 1, "HTTP/2.0 429 Status\r\nRetry-After: 60\r\n\r\n{}", "")
        outage = subprocess.CompletedProcess([], 1, "HTTP/2.0 503 Status\n\n{}", "")
        reset = subprocess.CompletedProcess([], 1, "", "read tcp: connection reset by peer")
        for result, wait in ((throttled, 60), (outage, None), (reset, None)):
            io = AllocationIO(Mock(return_value=result), clock=lambda: 0, sleep=Mock(), rand=lambda: 0.0)
            with self.subTest(wait=wait), self.assertRaises(TransientRead) as raised:
                AllocationGitHub("fixture", io).request("pulls/1")
            self.assertEqual(ci_retry.requested_wait(raised.exception), wait)
            self.assertEqual(ci_retry.classify(raised.exception), ci_retry.TRANSIENT)

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

    def test_settlement_reserve_is_the_full_wait_ladder_and_the_verification_reserve(self):
        # A push that uses its whole bound still leaves 1 + 2 + 4 + 8 s of waits
        # and the verification reserve for the last read and owner readbacks.
        self.assertEqual((SETTLE_WAITS, VERIFY_RESERVE_SECONDS), ((1, 2, 4, 8), 5))
        self.assertEqual((PUSH_SETTLEMENT_SECONDS, PUSH_RESERVE_SECONDS), (20, 21))

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

    def test_only_a_started_push_marks_the_remote_as_possibly_written(self):
        runner = Mock(return_value=Mock(returncode=0, stdout=""))
        io = AllocationIO(runner, clock=lambda: 0)
        io.read(["git", "ls-remote", "fixture"])
        io.write(["git", "mktag"])
        self.assertFalse(io.push_started)
        # A push the budget refuses never starts, so nothing remote can exist.
        late = AllocationIO(runner, clock=Mock(side_effect=[0, ALLOCATION_SECONDS - PUSH_RESERVE_SECONDS + 0.5]))
        with self.assertRaisesRegex(Refusal, "deadline"):
            late.write(["git", "push", "fixture"])
        self.assertFalse(late.push_started)
        for push in (runner, Mock(side_effect=subprocess.TimeoutExpired("git", PUSH_SECONDS))):
            started = AllocationIO(push, clock=lambda: 0)
            try:
                started.write(["git", "push", "fixture"])
            except UncertainPush:
                pass
            self.assertTrue(started.push_started)
            started.runner = runner
            started.write(["git", "mktag"])  # A later local write never clears it.
            self.assertTrue(started.push_started)

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
