"""A later run recovers displaced merges without coalescing their labels."""
import json
import unittest
from unittest.mock import Mock

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
        self.assertTrue(recheck(THIRD, api))
        run = {"head_sha": THIRD, "event": "push", "head_branch": "develop",
               "path": ".github/workflows/staging-cd.yml", "status": "in_progress"}
        for updates in ({}, {"status": "completed", "conclusion": "success"}):
            api.request.side_effect = [reference, {"workflow_runs": [dict(run, **updates)]}]
            self.assertTrue(recheck(SECOND, api))
        for runs in ([], [dict(run, status="completed", conclusion="failure")], [dict(run, head_sha=FIRST)]):
            api.request.side_effect = [reference, {"workflow_runs": runs}]
            with self.assertRaises(Refusal):
                recheck(SECOND, api)

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

    def test_two_displaced_pending_pushes_are_allocated_in_first_parent_order(self):
        pending = []
        reservations = Mock()
        reservations.list.side_effect = lambda: list(pending)
        reservations.insert.side_effect = lambda value: pending.append(value) is None
        result = allocate_queue(self.queue(), self.api(), reservations, 10, 1)
        tasks = json.loads(result["include"][0]["tasks"])
        self.assertEqual([task["reservation"]["source"] for task in tasks], [FIRST, SECOND])
        self.assertEqual([task["reservation"]["version"] for task in tasks], ["0.11.0", "0.11.1"])
        self.assertEqual([task["reservation"]["bump"] for task in tasks], ["minor", "patch"])

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


if __name__ == "__main__":
    unittest.main()
