"""Real insert-only Git pushes settle once and preserve every burned version."""
import json
import subprocess
import unittest

import staging_cd_git_tests as fixtures
from staging_cd_allocate import allocate_queue
from staging_cd_allocation_io import AllocationIO
from staging_cd_allocation_snapshot import AllocationConflict, AllocationSnapshot, SnapshotAPI
from staging_cd_common import Refusal
from staging_cd_github import GitHubNotFound
from staging_cd_reservations import next_reservation, pending_name


class AllocationGitTests(unittest.TestCase):
    def setUp(self):
        self.fixture = fixtures.GitPublicationTests()
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        self.calls, self.ambiguous = [], False
        self.fixture.api.request = self.request
        self.store = AllocationSnapshot(self.fixture.api, "fixture", self.fixture.work,
                                        AllocationIO(self.runner))

    def request(self, path, **_kwargs):
        if path.startswith("git/ref/tags/"):
            reference = path.removeprefix("git/ref/")
            try:
                oid = self.fixture.git("rev-parse", "refs/" + reference, remote=True)
            except subprocess.CalledProcessError:
                raise GitHubNotFound("GitHub resource was not found") from None
            return {"ref": "refs/" + reference, "object": {"type": "tag", "sha": oid}}
        result = self.fixture.request(path)
        if path.startswith("git/tags/"):
            result["sha"] = path.removeprefix("git/tags/")
        return result

    def runner(self, args, **kwargs):
        self.calls.append(args)
        result = self.fixture.runner(args, **kwargs)
        if args[1] == "push" and self.ambiguous:
            return subprocess.CompletedProcess(args, 1, "", "")
        return result

    def plan_two(self):
        self.store.load()
        values = []
        for _ in range(2):
            value = next_reservation("web", self.fixture.source, self.fixture.tree, 42,
                                     "patch", 10, 1, [], values)
            self.store.insert(value)
            values.append(value)
        return values

    def test_real_tags_are_insert_only_and_verified_individually(self):
        values = self.plan_two()
        self.store.commit()
        self.assertEqual(self.fixture.publisher.list(), values)
        pushes = [args for args in self.calls if args[1] == "push"]
        self.assertEqual(len(pushes), 1)
        self.assertIn("--atomic", pushes[0])
        for push in pushes:
            self.assertEqual(len([arg for arg in push if ":refs/tags/" in arg]), 5)
            self.assertNotIn("--force", push)
        self.assertEqual(self.store.list(), values)

    def test_different_bumps_cannot_claim_the_same_predecessor(self):
        patch = self.plan_two()[0]
        other = AllocationSnapshot(self.fixture.api, "fixture", self.fixture.work, AllocationIO(self.runner))
        other.load()
        other.insert(dict(patch, run_id=11, bump="minor", version="0.11.0"))
        self.store.commit()
        with self.assertRaises(AllocationConflict):
            other.commit()
        self.assertEqual(self.fixture.publisher.list(), self.store.planned)
        self.assertEqual(other.list(), [])

    def test_newer_allocation_during_final_push_prevents_late_final_and_deploy(self):
        f = self.fixture
        first = f.reservation()
        f.publisher.insert(first)
        newer = f.git("commit-tree", f.tree, "-p", f.source, data="Second merge\\n")
        f.git("push", str(f.remote), newer + ":refs/heads/develop")
        self.store.load()
        next_version = next_reservation("web", newer, f.tree, 43, "patch", 2, 1, [], [first])
        self.store.insert(next_version)
        old_receipt = f.receipt(first)
        new_receipt = f.receipt(next_version)
        runner = f.publisher.runner
        def finish_out_of_order(args, **kwargs):
            if args[1] == "push" and args[-1].endswith(":refs/tags/" + old_receipt["tag"]):
                self.store.commit()
                self.assertTrue(f.publisher.finalize(new_receipt, lambda *_: True))
            return runner(args, **kwargs)
        f.publisher.runner = finish_out_of_order
        f.publisher.sleep = lambda _seconds: None
        with self.assertRaises(Refusal):
            f.publisher.finalize(old_receipt, lambda *_: True)
        self.assertEqual([tag["receipt"] for tag in f.api.tags("web")], [new_receipt])
        self.assertEqual(f.publisher.list(), [first])
        f.api.require_current_versions({"web": new_receipt})
        with self.assertRaisesRegex(Refusal, "Final version is absent"):
            f.api.require_current_versions({"web": old_receipt})

    def test_finalize_wins_outcome_then_allocator_replans_from_verified_final(self):
        f = self.fixture
        first = f.reservation()
        f.publisher.insert(first)
        newer = f.git("commit-tree", f.tree, "-p", f.source, data="Second merge\n")
        f.git("push", str(f.remote), newer + ":refs/heads/develop")
        self.store.load()
        later = next_reservation("web", newer, f.tree, 43, "patch", 2, 1, [], [first])
        self.store.insert(later)
        f.publisher.finalize(f.receipt(first), lambda *_: True)
        with self.assertRaises(AllocationConflict):
            self.store.commit()
        self.assertEqual(f.publisher.list(), [])
        refreshed = AllocationSnapshot(f.api, "fixture", f.work, AllocationIO(self.runner)).load()
        refreshed.insert(later)
        refreshed.commit()
        self.assertEqual(f.publisher.list(), [later])
        # The newer version has only a pending ref, which is enough to fence the old final.
        with self.assertRaisesRegex(Refusal, "superseded"):
            f.api.require_current_versions({"web": f.receipt(first)})

    def test_first_collision_after_snapshot_stops_without_retry(self):
        values = self.plan_two()
        collision = dict(values[0], run_id=20)
        self.fixture.publisher.insert(collision)
        with self.assertRaises(Refusal):
            self.store.commit()
        self.assertEqual(self.fixture.publisher.list(), [collision])
        self.assertEqual(len([args for args in self.calls if args[1] == "push"]), 1)
        self.assertNotIn(pending_name(values[1]), [pending_name(value) for value in self.fixture.publisher.list()])

    def test_later_collision_rejects_entire_batch_without_partial_reservations(self):
        values = self.plan_two()
        collision = dict(values[1], run_id=20)
        self.fixture.publisher.insert(collision)
        with self.assertRaisesRegex(Refusal, "readback differs"):
            self.store.commit()
        self.assertEqual(self.fixture.publisher.list(), [collision])
        self.assertEqual(self.store.list(), [])
        self.assertEqual(len([args for args in self.calls if args[1] == "push"]), 1)
        with self.assertRaisesRegex(Refusal, "already attempted"):
            self.store.commit()
        self.assertEqual(len([args for args in self.calls if args[1] == "push"]), 1)

    def test_lost_success_response_settles_exact_objects_without_repeating_push(self):
        values = self.plan_two()
        self.ambiguous = True
        self.store.commit()
        self.assertEqual(self.fixture.publisher.list(), values)
        self.assertEqual(len([args for args in self.calls if args[1] == "push"]), 1)

    def test_overlapping_finalizer_resumes_or_skips_without_new_reservation(self):
        f = self.fixture
        value = f.reservation()
        self.assertTrue(f.publisher.insert(value))
        self.store.load()
        receipt = f.receipt(value)
        self.assertTrue(f.publisher.finalize(receipt, lambda *_: True))
        def request(path, **kwargs):
            if path == "actions/runs/1/attempts/1":
                return {"id": 1, "run_attempt": 1, "event": "push", "head_branch": "develop",
                        "repository": {"full_name": "ChronoAIProject/cma"}, "status": "completed"}
            return self.request(path, **kwargs)
        f.api.request = request
        api = SnapshotAPI(f.api, self.store)
        queue = {"head": f.source, "event_source": f.source, "changes": [
            {"source": f.source, "tree": f.tree, "deployables": ["cma-web"]}]}
        tasks = allocate_queue(queue, api, self.store, 2, 1)["include"]
        self.assertEqual(json.loads(tasks[0]["tasks"]), [{"component": "web", "mode": "resume", "receipt": receipt}])
        queue["event_source"] = "e" * 40
        self.assertEqual(allocate_queue(queue, api, self.store, 2, 1), {"include": []})
        self.store.commit()
        self.assertEqual(self.store.planned, [])
        self.assertEqual(f.publisher.list(), [])
        self.assertFalse(any(call[1] == "push" for call in self.calls))
        full = [call for call in self.calls if call[1] == "ls-remote" and "refs/tags/*" in call]
        self.assertEqual(len(full), 1)

    def test_missing_pending_and_final_is_uncertain(self):
        value = self.fixture.reservation()
        self.fixture.publisher.insert(value)
        self.store.load()
        self.assertIsNone(self.store.final_after_owner(value))
        self.fixture.git("update-ref", "-d", "refs/tags/" + pending_name(value), remote=True)
        with self.assertRaisesRegex(Refusal, "settlement is uncertain"):
            self.store.final_after_owner(value)

    def test_terminal_final_must_match_the_full_reservation(self):
        value = self.fixture.reservation()
        self.fixture.publisher.insert(value)
        self.store.load()
        self.fixture.publisher.finalize(self.fixture.receipt(value), lambda *_: True)
        with self.assertRaisesRegex(Refusal, "differs from final receipt"):
            self.store.final_after_owner(dict(value, run_id=2))

    def test_coexisting_final_and_pending_must_agree_before_planning(self):
        f = self.fixture
        value = f.reservation()
        f.publisher.insert(value)
        receipt = f.receipt(value)
        f.publisher.finalize(receipt, lambda *_: True)
        f.publisher.insert(value)  # Model a snapshot before the finalizer deletes pending.
        self.store.load()
        self.assertEqual(self.store.finals[0]["receipt"], receipt)
        f.git("update-ref", "-d", "refs/tags/" + pending_name(value), remote=True)
        f.publisher.insert(dict(value, run_id=2))
        conflicting = AllocationSnapshot(f.api, "fixture", f.work, AllocationIO(self.runner))
        with self.assertRaisesRegex(Refusal, "differs from final receipt"):
            conflicting.load()
        self.assertFalse(conflicting.loaded)
