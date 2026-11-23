"""An unreserved displaced source has no version to burn."""
import contextlib
import io
import json
import os
import time
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

import yaml

import staging_cd_recheck

from staging_cd_allocate import allocate_queue, run_active
from staging_cd_common import Refusal
from staging_cd_history import DevelopHistory
from staging_cd_queue import first_parent, reconcile
from staging_cd_recheck import recheck
from staging_cd_receipts_tests import fixture_receipt, fixture_tag
from staging_cd_reservations import next_reservation

ADOPTION, FIRST, SECOND, THIRD = (letter * 40 for letter in "abcd")


class QueueTests(unittest.TestCase):
    def test_new_head_requires_a_successful_or_live_successor_run(self):
        api = Mock()
        reference = {"ref": "refs/heads/develop", "object": {"type": "commit", "sha": THIRD}}
        api.request.return_value = reference
        api.deadline = None
        self.assertTrue(recheck(THIRD, api, sleep=Mock(side_effect=AssertionError("no wait expected"))))
        run = {"head_sha": THIRD, "event": "push", "head_branch": "develop",
               "path": ".github/workflows/staging-cd.yml", "status": "in_progress"}
        for updates in ({}, {"status": "completed", "conclusion": "success"},
                        *({"status": status} for status in ("queued", "waiting", "requested", "pending"))):
            api.request.side_effect = [reference, {"workflow_runs": [dict(run, **updates)]}]
            self.assertTrue(recheck(SECOND, api, sleep=Mock(side_effect=AssertionError("no wait expected"))))
        # A run that exists but did not succeed is refused at once, without waiting.
        for conclusion in ("failure", "cancelled", "timed_out", "startup_failure"):
            api.request.side_effect = [reference, {"workflow_runs": [dict(run, status="completed", conclusion=conclusion)]}]
            sleep = Mock()
            with self.assertRaisesRegex(Refusal, "no active or successful"):
                recheck(SECOND, api, sleep=sleep)
            sleep.assert_not_called()
        # No visible run, even after every bounded wait, is refused.
        for runs in ([], [dict(run, head_sha=FIRST)], [dict(run, path=".github/workflows/ci.yml")],
                     [dict(run, head_branch="main")]):
            polls = len(staging_cd_recheck.RUN_WAITS) + 1
            api.request.side_effect = [reference, {"workflow_runs": runs}] * polls
            sleep = Mock()
            with self.assertRaisesRegex(Refusal, "never showed a reconciliation run"):
                recheck(SECOND, api, sleep=sleep)
            self.assertEqual([call.args[0] for call in sleep.call_args_list], list(staging_cd_recheck.RUN_WAITS))

    def test_recheck_waits_for_a_just_pushed_head_to_get_its_run(self):
        # Run 44965203978: the head moved at 00:09:33Z and its run, created the
        # same second, was not yet listed when the recheck read it.
        api = Mock()
        reference = {"ref": "refs/heads/develop", "object": {"type": "commit", "sha": THIRD}}
        run = {"head_sha": THIRD, "event": "push", "head_branch": "develop",
               "path": ".github/workflows/staging-cd.yml", "status": "queued"}
        api.request.side_effect = [reference, {"workflow_runs": []}, reference, {"workflow_runs": []},
                                   reference, {"workflow_runs": [run]}]
        sleep = Mock()
        self.assertTrue(recheck(SECOND, api, sleep=sleep))
        self.assertEqual([call.args[0] for call in sleep.call_args_list], list(staging_cd_recheck.RUN_WAITS[:2]))

    def test_recheck_follows_develop_when_it_moves_again_while_waiting(self):
        api = Mock()
        third = {"ref": "refs/heads/develop", "object": {"type": "commit", "sha": THIRD}}
        fourth = {"ref": "refs/heads/develop", "object": {"type": "commit", "sha": "9" * 40}}
        run = {"head_sha": "9" * 40, "event": "push", "head_branch": "develop",
               "path": ".github/workflows/staging-cd.yml", "status": "in_progress"}
        api.request.side_effect = [third, {"workflow_runs": []}, fourth, {"workflow_runs": [run]}]
        self.assertTrue(recheck(SECOND, api, sleep=Mock()))

    def test_recheck_never_sleeps_into_the_read_deadline(self):
        api = Mock()
        reference = {"ref": "refs/heads/develop", "object": {"type": "commit", "sha": THIRD}}
        api.request.side_effect = [reference, {"workflow_runs": []}] * 6
        now = [100.0]
        api.deadline = 100.0 + 2 + 4 + staging_cd_recheck.READ_RESERVE_SECONDS + 1
        def sleep(seconds):
            now[0] += seconds
        with self.assertRaisesRegex(Refusal, "never showed a reconciliation run"):
            recheck(SECOND, api, sleep=sleep, clock=lambda: now[0])
        # 2 and 4 fit; 8 would end inside the read reserve, so it is not taken.
        self.assertEqual(now[0], 106.0)

    def test_recheck_waits_fit_inside_its_budget(self):
        # Reads are bounded separately by the client deadline; the waits alone
        # must leave most of the budget for those reads.
        self.assertLessEqual(sum(staging_cd_recheck.RUN_WAITS), staging_cd_recheck.RECHECK_SECONDS / 2)

    def test_recheck_reads_refuse_before_the_job_timeout(self):
        jobs = yaml.safe_load((Path(__file__).resolve().parents[1] / "workflows/staging-cd.yml").read_text())["jobs"]
        # Checkout and interpreter start keep at least a minute of the job.
        self.assertLessEqual(staging_cd_recheck.RECHECK_SECONDS + 60, jobs["recheck"]["timeout-minutes"] * 60)
        env = {"CD_QUEUE": json.dumps({"head": THIRD}), "GH_TOKEN": "fixture"}
        with patch.dict(os.environ, env, clear=True), patch.object(staging_cd_recheck, "event_identity"), \
                patch.object(staging_cd_recheck, "GitHub") as github, \
                patch.object(staging_cd_recheck, "recheck", return_value=True), \
                contextlib.redirect_stdout(io.StringIO()):
            started = time.monotonic()
            self.assertEqual(staging_cd_recheck.main(), 0)
        deadline = github.call_args.kwargs["deadline"]
        self.assertLessEqual(started + staging_cd_recheck.RECHECK_SECONDS, deadline)
        self.assertLessEqual(deadline, time.monotonic() + staging_cd_recheck.RECHECK_SECONDS)

    def queue(self):
        return {"head": THIRD, "event_source": THIRD, "changes": [
            {"source": FIRST, "tree": "e" * 40, "deployables": ["cma-web"]},
            {"source": SECOND, "tree": "f" * 40, "deployables": ["cma-web"]}]}

    def api(self):
        api = Mock()
        api.tags.return_value = []
        api.history.return_value = DevelopHistory(THIRD, [THIRD, SECOND, FIRST, ADOPTION])
        def request(path):
            first = FIRST in path or path == "pulls/1"
            source, number, level = (FIRST, 1, "minor") if first else (SECOND, 2, "patch")
            pr = {"number": number, "merged": True, "merged_at": "fixture", "merge_commit_sha": source,
                  "base": {"ref": "develop", "repo": {"full_name": "ChronoAIProject/cma"}},
                  "labels": [{"name": "version-upgrade:" + level}]}
            return [pr] if path.startswith("commits/") else pr
        api.request.side_effect = request
        return api

    def test_only_the_newest_unreserved_source_owns_a_version_and_a_build(self):
        pending = []
        reservations = Mock()
        reservations.list.side_effect = lambda: list(pending)
        reservations.insert.side_effect = lambda value: pending.append(value) is None
        result = allocate_queue(self.queue(), self.api(), reservations, 10, 1)
        tasks = json.loads(result["include"][0]["tasks"])
        self.assertEqual([task["reservation"]["source"] for task in tasks], [SECOND])
        self.assertEqual([item["source"] for item in pending], [SECOND])
        self.assertEqual([item["version"] for item in pending], ["0.10.1"])
        self.assertEqual([item["bump"] for item in pending], ["patch"])

    def test_noop_has_no_label_request_or_reservation(self):
        api, reservations = self.api(), Mock()
        reservations.list.return_value = []
        queue = {"head": THIRD, "changes": [{"source": FIRST, "tree": "e" * 40, "deployables": []}]}
        self.assertEqual(allocate_queue(queue, api, reservations, 10, 1), {"include": []})
        api.request.assert_not_called()
        reservations.insert.assert_not_called()

    def test_final_same_merge_resumes_without_any_allocation_or_label_read(self):
        receipt = fixture_receipt(source=THIRD)
        api, reservations = self.api(), Mock()
        api.tags.return_value = [fixture_tag(receipt)]
        reservations.list.return_value = []
        queue = {"head": THIRD, "event_source": THIRD, "changes": [
            {"source": THIRD, "tree": "b" * 40, "deployables": ["cma-web"]}]}
        result = allocate_queue(queue, api, reservations, 10, 2)
        self.assertEqual(json.loads(result["include"][0]["tasks"]), [{"component": "web", "mode": "resume", "receipt": receipt}])
        api.request.assert_not_called()
        reservations.insert.assert_not_called()

    def test_active_other_attempt_is_not_allocated_twice_and_unknown_liveness_fails(self):
        api, reservations = self.api(), Mock()
        owned = next_reservation("web", FIRST, "e" * 40, 1, "patch", 9, 1, [], [])
        reservations.list.return_value = [owned]
        run = {"id": 9, "run_attempt": 1, "event": "push", "head_branch": "develop",
               "repository": {"full_name": "ChronoAIProject/cma"}, "status": "in_progress"}
        api.request.side_effect = None
        api.request.return_value = run
        queue = self.queue()
        queue["changes"] = queue["changes"][:1]
        self.assertEqual(allocate_queue(queue, api, reservations, 10, 1), {"include": []})
        reservations.insert.assert_not_called()
        api.request.return_value = dict(run, status="unknown")
        with self.assertRaises(Refusal):
            run_active(api, owned)

    def test_first_parent_reconciliation_includes_each_merge_and_rejects_side_branch_anchor(self):
        def command(_root, *args):
            return "\n".join((THIRD, SECOND, FIRST, ADOPTION)) if args[0] == "rev-list" else "e" * 40
        self.assertEqual(first_parent(None, ADOPTION, THIRD, command), [ADOPTION, FIRST, SECOND, THIRD])
        self.assertEqual(first_parent(None, THIRD, THIRD, command), [THIRD])
        selector = Mock(side_effect=[["cma-web"], ["cma-web"], [], ["cma-agent-manager", "cma-event-gateway"]])
        result = reconcile(None, ADOPTION, THIRD, selector, command)
        self.assertEqual([row["source"] for row in result], [ADOPTION, FIRST, THIRD])
        with self.assertRaises(Refusal):
            first_parent(None, "f" * 40, THIRD, command)



    def test_history_walk_is_bounded_and_requires_the_adoption_commit(self):
        command = Mock(return_value=THIRD + '\n' + SECOND)
        self.assertEqual(first_parent(None, SECOND, THIRD, command), [SECOND, THIRD])
        command.assert_called_once_with(None, 'rev-list', '--first-parent', '--max-count=2000', THIRD)
        with self.assertRaises(Refusal):
            first_parent(None, FIRST, THIRD, command)


if __name__ == "__main__":
    unittest.main()
