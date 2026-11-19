"""Real insert-only Git pushes settle once and preserve every burned prefix."""
import json
import subprocess
import unittest

import staging_cd_git_tests as fixtures
from staging_cd_allocate import allocate_queue
from staging_cd_allocation_io import AllocationIO
from staging_cd_allocation_snapshot import AllocationSnapshot, SnapshotAPI
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
        self.assertEqual(len(pushes), 2)
        for push in pushes:
            self.assertEqual(len([arg for arg in push if ":refs/tags/" in arg]), 1)
            self.assertNotIn("--force", push)
        self.assertEqual(self.store.list(), values)

    def test_first_collision_after_snapshot_stops_without_retry(self):
        values = self.plan_two()
        collision = dict(values[0], run_id=20)
        self.fixture.publisher.insert(collision)
        with self.assertRaises(Refusal):
            self.store.commit()
        self.assertEqual(self.fixture.publisher.list(), [collision])
        self.assertEqual(len([args for args in self.calls if args[1] == "push"]), 1)
        self.assertNotIn(pending_name(values[1]), [pending_name(value) for value in self.fixture.publisher.list()])

    def test_later_collision_keeps_verified_prefix_burned_without_repeating_it(self):
        values = self.plan_two()
        collision = dict(values[1], run_id=20)
        self.fixture.publisher.insert(collision)
        with self.assertRaisesRegex(Refusal, "readback differs"):
            self.store.commit()
        self.assertEqual(self.fixture.publisher.list(), [values[0], collision])
        self.assertEqual(self.store.list(), [values[0]])
        self.assertEqual(len([args for args in self.calls if args[1] == "push"]), 2)
        with self.assertRaisesRegex(Refusal, "already attempted"):
            self.store.commit()
        self.assertEqual(len([args for args in self.calls if args[1] == "push"]), 2)

    def test_lost_success_response_settles_exact_objects_without_repeating_push(self):
        values = self.plan_two()
        self.ambiguous = True
        self.store.commit()
        self.assertEqual(self.fixture.publisher.list(), values)
        self.assertEqual(len([args for args in self.calls if args[1] == "push"]), 2)

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
        full = [call for call in self.calls if call[1] == "ls-remote" and len(call) == 5]
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
