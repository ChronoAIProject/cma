"""The allocator exposes a matrix only after every reservation is settled."""
import contextlib
import io
import json
import os
import subprocess
import time
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

import yaml

import staging_cd_allocate as allocation
from staging_cd_allocate_progress import allocation_progress
from staging_cd_allocation_fixture import AllocationServer
from staging_cd_allocation_snapshot import AllocationConflict, AllocationSnapshot, SnapshotAPI
from staging_cd_allocation_io import ALLOCATION_SECONDS, AllocationIO
from staging_cd_common import Refusal
from staging_cd_github import GitHub
from staging_cd_history import DevelopHistory
from staging_cd_outcome_tests import legacy
from staging_cd_reservations import claim_ref, next_reservation, pending_name

OUTAGE = "fatal: unable to access 'https://github.com/ChronoAIProject/cma.git/': The requested URL returned error: 502\n"


def github_refusal(status, headers=""):
    """The refusal a GitHub API read raises for an HTTP status, built by the real response check."""
    result = subprocess.CompletedProcess([], 1, f"HTTP/2.0 {status} Status\n{headers}\n{{}}", "")
    try:
        GitHub.response("commits/x/pulls", None, result, False)
    except Refusal as error:
        return error
    raise AssertionError("the response was accepted")


def retries(output):
    return [line for line in output.splitlines() if line.startswith("RETRY allocation")]


class AllocationEntrypointTests(unittest.TestCase):
    def execute(self, *, fail_after_first=False, labels=None, command=None, failing_read=None):
        """Run the allocation step against the fixture remote on a fake clock.

        `command(server, args, kwargs, advance)` replaces the git runner of every
        attempt and `failing_read(path)` may return a refusal to raise for an API
        read. Each attempt gets its own budget on the shared fake clock, and the
        progress reporter reads the same clock, so its elapsed seconds are exact.
        A rerun's wait advances that clock too, so no regression can block on
        a real sleep; the jitter is fixed at its shortest wait.
        """
        server = AllocationServer(0, components=("am", "web"))
        stores, output, events, outputs = [], io.StringIO(), [], {}
        request = server.request
        now = [0.0]

        def advance(seconds):
            now[0] += seconds

        def bounded():
            return AllocationIO(clock=lambda: now[0], sleep=advance, rand=lambda: 0.0)

        attempts = allocation.allocate_attempts

        def faked(*args, **kwargs):
            return attempts(*args, sleep=advance, rand=lambda: 0.0, **kwargs)

        def read(path, **kwargs):
            if path.startswith("git/ref/tags/"):
                events.append("readback")
            failure = failing_read(path) if failing_read else None
            if failure is not None:
                raise failure
            result = request(path, **kwargs)
            if fail_after_first and "/cma-web/pending/" in path:
                return dict(result, object={"type": "tag", "sha": "0" * 40})
            if labels is not None and path.startswith("pulls/"):
                return dict(result, labels=[{"name": name} for name in labels])
            return result

        def store(api, token, root, bounded_io, *, mark):
            bounded_io.runner = server.command if command is None else (
                lambda args, **kwargs: command(server, args, kwargs, advance))
            snapshot = AllocationSnapshot(api, token, root, bounded_io, mark=mark)
            stores.append(snapshot)
            return snapshot

        def emit(_env, key, value):
            events.append(key)
            outputs[key] = value
            if key == "matrix":
                self.assertEqual(len(json.loads(value)["include"]), 2)

        server.request = read
        env = {"GITHUB_EVENT_NAME": "push", "GITHUB_RUN_ID": "999", "GITHUB_RUN_ATTEMPT": "1",
               "GH_TOKEN": "fixture", "GITHUB_SHA": server.history().head, "CD_QUEUE": json.dumps(server.queue())}
        with patch.dict(os.environ, env, clear=True), contextlib.redirect_stdout(output), \
                patch.object(allocation, "event_identity"), patch.object(allocation, "GitHub", return_value=server), \
                patch.object(allocation, "Reservations", side_effect=store), \
                patch.object(allocation, "AllocationIO", side_effect=bounded), \
                patch.object(allocation, "allocation_progress", lambda: allocation_progress(lambda: now[0])), \
                patch.object(allocation, "allocate_attempts", faked), \
                patch.object(allocation, "output", side_effect=emit) as emitted:
            status = allocation.main([])
        self.outputs, self.stores = outputs, stores
        return status, server, stores[0], legacy(output.getvalue()), events, emitted

    def gate(self, outputs):
        """The settlement gate step, given the allocation step's outputs as the workflow passes them."""
        output = io.StringIO()
        env = {"CD_ALLOCATION_REFUSED": outputs.get("refused", ""), "CD_ALLOCATION_SITE": outputs.get("site", "")}
        with patch.dict(os.environ, env, clear=True), contextlib.redirect_stdout(output):
            status = allocation.main(["--require-settled"])
        legacy(output.getvalue())  # One explained line and its annotation.
        return status, output.getvalue().splitlines()[0]

    def test_success_emits_matrix_after_both_exact_ref_readbacks(self):
        status, server, store, output, events, emitted = self.execute()
        self.assertEqual(status, 0)
        self.assertEqual(events, ["readback", "readback", "matrix", "changed"])
        self.assertEqual(len(server.refs), 4)
        self.assertEqual(store.list(), store.planned)
        self.assertIn("phase=complete count=1", output)
        self.assertTrue(output.endswith("PASS allocation\n"))
        self.assertEqual(emitted.call_count, 2)

    def test_partial_settlement_stops_at_the_gate_without_a_matrix_or_repeating_prior_push(self):
        status, server, store, output, events, emitted = self.execute(fail_after_first=True)
        # A transient refusal ends the allocation step successfully, without a
        # matrix, so that only its settlement gate fails the job.
        self.assertEqual(status, 0)
        self.assertEqual(events, ["readback", "readback", "transient", "refused", "site"])
        self.assertEqual({key: value for key, value in self.outputs.items() if key != "site"},
                         {"transient": "true", "refused": "RESERVATION_VERIFY_FAILED"})
        self.assertRegex(self.outputs["site"], r"\Astaging_cd_allocation_snapshot\.py:\d+\Z")
        self.assertEqual(len(store.list()), 0)
        self.assertEqual(len(server.refs), 4)
        self.assertEqual(len([args for args in server.commands if args[1] == "push"]), 1)
        self.assertNotIn("phase=complete", output)
        self.assertIn("PROGRESS allocation component=web phase=readback-conflict count=1 elapsed_seconds=0\n", output)
        # Both readbacks run in parallel, so either one may report last.
        self.assertTrue(output.endswith(" elapsed_seconds=0\nFAIL allocation\n"))
        self.assertNotIn("fixture", output)
        # The push had started, so allocation is not run again in this job.
        self.assertEqual((retries(output), len(self.stores)), ([], 1))
        self.assertEqual(self.gate(self.outputs),
                         (1, f"FAIL allocation RESERVATION_VERIFY_FAILED at={self.outputs['site']} cause=transient"))

    def test_a_transient_read_before_the_push_reruns_allocation_once_and_succeeds(self):
        failures = [3]  # Enough to exhaust the read's own three attempts.

        def command(server, args, kwargs, _advance):
            if args[1] == "ls-remote" and failures[0]:
                failures[0] -= 1
                return subprocess.CompletedProcess(args, 128, "", OUTAGE)
            return server.command(args, **kwargs)
        status, server, first, output, events, _ = self.execute(command=command)
        self.assertEqual(status, 0)
        self.assertEqual(events, ["readback", "readback", "matrix", "changed"])
        self.assertEqual(retries(output), ["RETRY allocation reason=READ_FAILED attempt=2/2 wait=10.0s"])
        self.assertTrue(output.endswith("PASS allocation\n"))
        # A fresh budget and a fresh, complete inventory; the push ran once.
        self.assertEqual(len(self.stores), 2)
        self.assertIsNot(self.stores[0].io, self.stores[1].io)
        self.assertFalse(first.loaded)
        self.assertEqual(len([args for args in server.commands if args[1] == "push"]), 1)
        self.assertEqual(len(server.refs), 4)

    def test_lagging_develop_advertisement_cannot_complete_allocation_without_work(self):
        current = []

        def command(server, args, kwargs, _advance):
            if args[1] == "ls-remote":
                if not current:
                    older = "f" * 40
                    head = server.history().head
                    current.append(DevelopHistory(head, [head, older]))
                    server._history = DevelopHistory(older, [older])
                else:
                    server._history = current[0]
            return server.command(args, **kwargs)

        status, server, first, output, events, _ = self.execute(command=command)
        self.assertEqual(status, 0)
        self.assertEqual(events, ["readback", "readback", "matrix", "changed"])
        self.assertEqual(retries(output), ["RETRY allocation reason=DEVELOP_ADVERTISEMENT_UNSETTLED attempt=2/2 wait=10.0s"])
        self.assertFalse(first.loaded)
        self.assertEqual(len([args for args in server.commands if args[1] == "push"]), 1)
        self.assertEqual({item["source"] for item in self.stores[1].planned}, {server.sources[-1]})

    def test_a_listing_torn_by_a_concurrent_push_is_read_again_and_succeeds(self):
        # git ls-remote is not an atomic snapshot. While another job's atomic
        # push lands, one listing can show its claim but not its reservation.
        torn = []

        def command(server, args, kwargs, _advance):
            if args[1] == "ls-remote" and not torn:
                rival = next_reservation("am", server.history().head, "e" * 40, 1, "patch", 50, 1, [], [])
                oid = server.object(rival)
                torn.append(rival)
                server.refs[claim_ref(rival).removeprefix("refs/tags/")] = oid
                listed = server.command(args, **kwargs)
                server.refs[pending_name(rival)] = oid  # The rest of the push lands after the listing.
                return listed
            return server.command(args, **kwargs)
        status, server, first, output, events, _ = self.execute(command=command)
        self.assertEqual(status, 0)
        self.assertEqual(events, ["readback", "readback", "matrix", "changed"])
        self.assertEqual(retries(output), ["RETRY allocation reason=INVENTORY_CLAIM_ORPHANED attempt=2/2 wait=10.0s"])
        self.assertFalse(first.loaded)
        # The rival's version stays burned; this run reserves the next one.
        planned = {item["component"]: item["previous"] for item in self.stores[1].planned}
        self.assertEqual(planned["am"], torn[0]["version"])
        self.assertEqual(len(server.refs), 6)

    def test_a_second_transient_refusal_stops_at_the_gate_as_before(self):
        def command(server, args, kwargs, _advance):
            if args[1] == "ls-remote":
                return subprocess.CompletedProcess(args, 128, "", OUTAGE)
            return server.command(args, **kwargs)
        status, server, _, output, events, _ = self.execute(command=command)
        self.assertEqual(status, 0)
        self.assertEqual(events, ["transient", "refused", "site"])
        self.assertEqual(self.outputs["refused"], "READ_FAILED")
        self.assertEqual(retries(output), ["RETRY allocation reason=READ_FAILED attempt=2/2 wait=10.0s"])
        self.assertTrue(output.endswith("FAIL allocation\n"))
        self.assertEqual(len(self.stores), 2)
        self.assertEqual(server.refs, {})
        self.assertFalse(any(args[1] == "push" for args in server.commands))
        self.assertEqual(self.gate(self.outputs),
                         (1, f"FAIL allocation READ_FAILED at={self.outputs['site']} cause=transient"))

    def test_a_deadline_before_the_push_reruns_allocation_with_a_fresh_budget(self):
        spent = []

        def command(server, args, kwargs, advance):
            if args[1] == "mktag" and not spent:
                spent.append(True)
                advance(ALLOCATION_SECONDS - 10)  # Too little is left to start the push.
            return server.command(args, **kwargs)
        status, server, _, output, _, _ = self.execute(command=command)
        self.assertEqual(status, 0)
        self.assertEqual(retries(output), ["RETRY allocation reason=DEADLINE_EXCEEDED attempt=2/2 wait=10.0s"])
        # The second budget starts in full after the spent one and the rerun's wait.
        self.assertEqual(self.stores[1].io.deadline - self.stores[0].io.deadline,
                         ALLOCATION_SECONDS - 10 + allocation.RETRY_WAIT_SECONDS[0])
        self.assertEqual(len([args for args in server.commands if args[1] == "push"]), 1)
        self.assertEqual(len(server.refs), 4)

    def test_an_uncertain_push_is_settled_or_gated_but_never_rerun(self):
        pushes = []

        def command(server, args, kwargs, _advance):
            if args[1] == "push":
                pushes.append(args)  # The server never commits it, so settlement runs out of budget.
                raise subprocess.TimeoutExpired("git", kwargs["timeout"])
            return server.command(args, **kwargs)
        status, server, store, output, _, _ = self.execute(command=command)
        self.assertEqual((status, self.outputs["refused"]), (0, "WRITE_UNCERTAIN"))
        self.assertEqual((retries(output), len(self.stores)), ([], 1))
        self.assertTrue(store.io.push_started)
        self.assertEqual((len(pushes), server.refs), (1, {}))
        self.assertEqual(self.gate(self.outputs),
                         (1, f"FAIL allocation WRITE_UNCERTAIN at={self.outputs['site']} cause=transient"))

    def test_a_declined_push_fails_the_step_for_an_operator_without_a_rerun(self):
        def command(server, args, kwargs, _advance):
            server.rejection = (1, "!\tx:refs/tags/x\t[remote rejected] (pre-receive hook declined)\n", "")
            return server.command(args, **kwargs)
        status, server, _, output, _, emitted = self.execute(command=command)
        self.assertEqual(status, 1)
        emitted.assert_not_called()
        self.assertEqual(retries(output), [])
        self.assertTrue(output.endswith("FAIL allocation\nFAIL allocation RESERVATION_PUSH_REJECTED\n"))
        self.assertEqual(len([args for args in server.commands if args[1] == "push"]), 1)

    def test_a_spent_primary_rate_limit_is_left_to_the_gate(self):
        # GitHub asks to wait until its quota resets, far beyond a rerun's wait:
        # repeating every tag read now could only fail again.
        spent = github_refusal(403, f"X-RateLimit-Remaining: 0\nX-RateLimit-Reset: {int(time.time()) + 3600}\n")
        status, server, _, output, events, _ = self.execute(
            failing_read=lambda path: spent if path.startswith("commits/") else None)
        self.assertEqual((status, events), (0, ["transient", "refused", "site"]))
        self.assertEqual(self.outputs["refused"], "GITHUB_READ_TRANSIENT")
        self.assertEqual((retries(output), len(self.stores), server.refs), ([], 1, {}))
        self.assertEqual(self.gate(self.outputs)[1].split(" at=")[0], "FAIL allocation GITHUB_READ_TRANSIENT")

    def test_a_rerun_waits_what_github_asked_for_up_to_its_longest_wait(self):
        queue, env = {"event_source": "a" * 40}, {}

        def owner_read(headers):
            # A wrapped read keeps its cause, so the request is still seen.
            try:
                raise Refusal("Reservation owner run could not be read") from github_refusal(403, headers)
            except Refusal as error:
                return error
        for first, reruns in ((github_refusal(403, "Retry-After: 15\n"), [15]),
                              (github_refusal(403, "Retry-After: 20\n"), [20]),
                              (github_refusal(403, "Retry-After: 21\n"), []),
                              (owner_read("Retry-After: 60\n"), []),
                              (owner_read("Retry-After: 3\n"), [10.0])):
            with self.subTest(first=allocation.refusal_code(first), reruns=reruns):
                sleep, retry = Mock(), Mock()
                with patch.object(allocation, "AllocationIO", side_effect=[Mock(push_started=False)] * 2), \
                        patch.object(allocation, "allocate_current", side_effect=[first, {"include": []}]):
                    if reruns:
                        allocation.allocate_attempts(queue, env, Mock(), retry, sleep=sleep, rand=lambda: 0.0)
                    else:
                        with self.assertRaises(Refusal):
                            allocation.allocate_attempts(queue, env, Mock(), retry, sleep=sleep, rand=lambda: 0.0)
                self.assertEqual([call.args[0] for call in sleep.call_args_list], reruns)
                self.assertEqual(retry.call_count, len(reruns))

    def test_a_rate_limited_github_read_is_rerun_and_a_denied_one_is_not(self):
        throttled = github_refusal(403, "X-RateLimit-Remaining: 0\nX-RateLimit-Reset: 0\n")
        failures = [throttled]
        status, server, _, output, _, _ = self.execute(
            failing_read=lambda path: failures.pop() if path.startswith("commits/") and failures else None)
        self.assertEqual(status, 0)
        self.assertEqual(retries(output), ["RETRY allocation reason=GITHUB_READ_TRANSIENT attempt=2/2 wait=10.0s"])
        self.assertEqual(len(server.refs), 4)
        denied = github_refusal(403)
        status, server, _, output, _, emitted = self.execute(
            failing_read=lambda path: denied if path.startswith("commits/") else None)
        self.assertEqual(status, 1)
        emitted.assert_not_called()
        self.assertEqual((retries(output), len(self.stores)), ([], 1))
        self.assertTrue(output.endswith("FAIL allocation\nFAIL allocation GITHUB_READ_DENIED\n"))
        self.assertEqual(server.refs, {})

    def test_unlabelled_merged_pr_reserves_patch_for_each_component(self):
        status, server, store, output, _, _ = self.execute(labels=["train"])
        self.assertEqual(status, 0)
        self.assertEqual(len(server.refs), 4)
        self.assertEqual({(item["component"], item["bump"]) for item in store.list()},
                         {("am", "patch"), ("web", "patch")})
        self.assertTrue(output.endswith("PASS allocation\n"))

    def test_conflicting_labels_fail_the_allocation_step_itself(self):
        status, server, _, output, _, emitted = self.execute(labels=["version-upgrade:major", "version-upgrade:patch"])
        self.assertEqual(status, 1)
        emitted.assert_not_called()
        self.assertEqual(server.refs, {})
        self.assertTrue(output.endswith("FAIL allocation\nFAIL allocation VERSION_LABEL_REQUIRED\n"))
        self.assertNotIn("VERSION_LABEL_REQUIRED", allocation.TRANSIENT_CODES)
        self.assertEqual((retries(output), len(self.stores)), ([], 1))

    def test_the_gate_reports_only_a_transient_code_and_refuses_unknown_arguments(self):
        for outputs, line in (({"refused": "WRITE_UNCERTAIN", "site": "staging_cd_allocation_snapshot.py:9"},
                               "FAIL allocation WRITE_UNCERTAIN at=staging_cd_allocation_snapshot.py:9 cause=transient"),
                              ({"refused": "READ_FAILED"}, "FAIL allocation READ_FAILED cause=transient"),
                              # Only a transient code can reach the gate; anything else is reported as unknown.
                              ({"refused": "VERSION_LABEL_REQUIRED"}, "FAIL allocation UNCLASSIFIED_REFUSAL cause=unknown"),
                              ({}, "FAIL allocation UNCLASSIFIED_REFUSAL cause=unknown")):
            with self.subTest(outputs=outputs):
                self.assertEqual(self.gate(outputs), (1, line))
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            self.assertEqual(allocation.main(["--unknown"]), 1)
        self.assertTrue(output.getvalue().startswith("FAIL allocation reason=refused"))

    def test_transient_codes_are_exactly_the_catalogued_transient_allocation_codes(self):
        catalogued = json.loads(Path(__file__).with_name("ci_outcomes.json").read_text())
        codes = catalogued["families"]["allocation"]["codes"]
        self.assertEqual(allocation.TRANSIENT_CODES, {code for code, entry in codes.items() if entry["cause"] == "transient"})
        self.assertLessEqual(allocation.TRANSIENT_CODES,
                             set(allocation.REFUSAL_CODES.values()) | allocation.GITHUB_READ_CAUSES)
        # The incident codes of an uncertain push and a spent budget are among them.
        self.assertLessEqual({"WRITE_UNCERTAIN", "READ_FAILED", "DEADLINE_EXCEEDED"}, allocation.TRANSIENT_CODES)
        # A rerun follows only a transient refusal, and never one that says a write may have committed.
        self.assertEqual(allocation.RETRIED_CODES, allocation.TRANSIENT_CODES - {"WRITE_UNCERTAIN"})
        # Durable causes are no longer catalogued as transient.
        for code in ("GITHUB_READ_DENIED", "RESERVATION_PUSH_REJECTED", "INVENTORY_CLAIM_CONFLICT",
                     "INVENTORY_TOO_LARGE"):
            self.assertEqual(codes[code]["cause"], "operator", code)
        # One listing can show a concurrent push's claim before its reservation.
        self.assertIn("INVENTORY_CLAIM_ORPHANED", allocation.RETRIED_CODES)
        self.assertEqual(codes["RESERVATION_PLAN_INVALID"]["cause"], "unknown")

    def test_one_rerun_waits_ten_to_twenty_seconds_and_fits_the_job_timeout(self):
        queue, env = {"event_source": "a" * 40}, {}
        for rand, push_started, first, reruns in ((0.0, False, Refusal("Allocation read failed"), [10.0]),
                                                  (1.0, False, Refusal("Reservation owner run could not be read"), [20.0]),
                                                  (0.5, True, Refusal("Allocation read failed"), []),
                                                  (0.5, False, Refusal("Allocation write settlement is uncertain"), []),
                                                  (0.5, False, Refusal("Release queue is invalid"), []),
                                                  (0.5, False, ValueError("private"), [])):
            with self.subTest(first=str(first), push_started=push_started):
                ios = [Mock(push_started=push_started), Mock(push_started=False)]
                sleep, retry = Mock(), Mock()
                with patch.object(allocation, "AllocationIO", side_effect=ios), \
                        patch.object(allocation, "allocate_current", side_effect=[first, {"include": []}]) as plan:
                    if reruns:
                        self.assertEqual(allocation.allocate_attempts(queue, env, Mock(), retry, sleep=sleep,
                                                                      rand=lambda: rand), {"include": []})
                    else:
                        with self.assertRaises(type(first)):
                            allocation.allocate_attempts(queue, env, Mock(), retry, sleep=sleep, rand=lambda: rand)
                self.assertEqual([call.args[0] for call in sleep.call_args_list], reruns)
                self.assertEqual(plan.call_count, 1 + len(reruns))
                if reruns:
                    retry.assert_called_once_with(allocation.refusal_code(first), 2, 2, reruns[0])
                    self.assertIs(plan.call_args.args[2], ios[1])
                else:
                    retry.assert_not_called()
        # Both attempts' budgets and the longest wait use at most half of the
        # allocate job's timeout, which checkout and routing also need.
        workflow = yaml.safe_load((Path(__file__).resolve().parents[1] / "workflows" / "staging-cd.yml").read_text())
        timeout = workflow["jobs"]["allocate"]["timeout-minutes"] * 60
        worst = allocation.ALLOCATION_ATTEMPTS * ALLOCATION_SECONDS + allocation.RETRY_WAIT_SECONDS[1]
        self.assertEqual((allocation.ALLOCATION_ATTEMPTS, worst), (2, 260))
        self.assertLessEqual(2 * worst, timeout)

    def test_replan_reloads_controller_proof_for_current_final(self):
        head = "a" * 40
        queue = {"head": head, "event_source": head, "changes": []}
        proofs = {"web": {"writer": head, "stamp": "web " + "a" * 64}}
        env = {"GH_TOKEN": "fixture", "GITHUB_RUN_ID": "999", "GITHUB_RUN_ATTEMPT": "1",
               "CD_STAGING_ENVIRONMENT": "staging"}
        first, second = Mock(), Mock()
        first.commit.side_effect = AllocationConflict()
        seen = []

        def allocate_queue(*args, read_proofs, **kwargs):
            seen.append(read_proofs())
            return {"include": []}

        with patch.object(allocation, "Reservations", side_effect=[first, second]), \
                patch.object(allocation, "GitHub"), patch.object(allocation, "allocate_queue", side_effect=allocate_queue), \
                patch.object(allocation, "available_proofs", side_effect=[{}, proofs]):
            self.assertEqual(allocation.allocate_current(queue, env, Mock(), Mock()), {"include": []})
        self.assertEqual(seen, [{}, proofs])
        for snapshot in (first, second):
            snapshot.load.assert_called_once_with(event_source=head)
            snapshot.commit.assert_called_once_with()

    def test_four_develop_merges_eighteen_seconds_apart_out_of_order(self):
        server = AllocationServer(3, reserved=False)
        changes = server.queue()["changes"]
        marks, plans, queues = Mock(), [], []
        io = AllocationIO(server.command)
        for index, arrival in enumerate((0, 18, 36, 54)):
            source = server.sources[index]
            server._history = DevelopHistory(source, list(reversed(server.sources[:index + 1])))
            queue = {"event_source": source, "head": source, "changes": changes[:index + 1]}
            queues.append(queue)
            if arrival == 54:
                break
            plan = AllocationSnapshot(server, "fixture", Path.cwd(), io).load()
            allocation.allocate_queue(queue, SnapshotAPI(server, plan), plan, 100 + index, 1)
            plans.append(plan)
            if arrival == 0:
                plan.commit()  # This run is still publishing at the fourth merge.

        # The second allocator wins while the fourth has a stale snapshot.
        # The third allocator reaches its write only after the fourth finishes.
        second = [plans[1]]
        def interleave(args, **kwargs):
            if args[1] == "push" and second:
                second.pop().commit()
            return server.command(args, **kwargs)
        io.runner = interleave
        env = {"GH_TOKEN": "fixture", "GITHUB_RUN_ID": "103", "GITHUB_RUN_ATTEMPT": "1"}
        with patch.object(allocation, "GitHub", return_value=server):
            result = allocation.allocate_current(queues[-1], env, io, marks)
            task, = json.loads(result["include"][0]["tasks"])
            self.assertEqual((task["reservation"]["version"], task["reservation"]["source"]),
                             ("0.10.3", server.sources[-1]))
            self.assertIn(unittest.mock.call("all", "replan"), marks.call_args_list)
            with self.assertRaises(AllocationConflict):
                plans[2].commit()
            self.assertEqual(allocation.allocate_current(queues[2], env, io, marks), {"include": []})
        final = AllocationSnapshot(server, "fixture", Path.cwd(), io).load()
        self.assertEqual([(r["version"], r["source"]) for r in final.list()],
                         [("0.10.1", server.sources[0]), ("0.10.2", server.sources[1]), ("0.10.3", server.sources[3])])
