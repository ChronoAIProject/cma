"""Real insert-only Git pushes settle once and preserve every burned version."""
import json
import subprocess
import unittest
from unittest.mock import Mock

import staging_cd_git_tests as fixtures
from staging_cd_allocate import allocate_queue
from staging_cd_allocation_io import AllocationIO
from staging_cd_allocation_snapshot import AllocationConflict, AllocationSnapshot, SnapshotAPI
from staging_cd_common import Refusal
from staging_cd_github import GitHubNotFound
from staging_cd_history import DevelopHistory
from staging_cd_receipts import canonical
from staging_cd_reservations import next_reservation, pending_name
from staging_cd_resolve import ObsoleteSource, verify
from staging_cd_versions import DEPLOYABLES

LANES = sorted(DEPLOYABLES)


class AllocationGitTests(unittest.TestCase):
    def setUp(self):
        self.calls, self.ambiguous = [], False
        self.fresh()

    def fresh(self):
        self.fixture = fixtures.GitPublicationTests()
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        self.fixture.api.request = self.request
        self.store = AllocationSnapshot(self.fixture.api, "fixture", self.fixture.work,
                                        AllocationIO(self.runner))
        return self.fixture

    def successor(self, component):
        """Reserve a predecessor, then plan its successor for a newer develop merge."""
        f = self.fresh()
        first = next_reservation(component, f.source, f.tree, 42, "patch", 1, 1, [], [])
        f.publisher.insert(first)
        newer = f.git("commit-tree", f.tree, "-p", f.source, data="Second merge\n")
        f.git("push", str(f.remote), newer + ":refs/heads/develop")
        self.store.load()
        later = next_reservation(component, newer, f.tree, 43, "patch", 2, 1, [], [first])
        self.store.insert(later)
        f.publisher.sleep = lambda _seconds: None
        return f, first, later

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
            # Two reservations and their claims; a started predecessor keeps its outcome.
            self.assertEqual(len([arg for arg in push if ":refs/tags/" in arg]), 4)
            self.assertFalse(any("cma-allocation-outcome/" in arg for arg in push))
            self.assertNotIn("--force", push)
        self.assertEqual(self.store.list(), values)

    def test_only_first_parent_advancement_marks_the_push_superseded(self):
        f = self.fixture
        newer = f.git("commit-tree", f.tree, "-p", f.source, data="Second develop push\n")
        f.git("update-ref", "HEAD", f.source)
        with self.assertRaisesRegex(Refusal, "Develop history has not reached the push"):
            self.store.load(event_source=newer)
        self.assertFalse(self.store.loaded)
        self.assertFalse(any(args[1] == "push" for args in self.calls))

        planner = f.root / "planner"
        f.git("clone", "--quiet", "--depth=1", "--branch", "develop", f.remote.as_uri(), str(planner))
        self.assertEqual(f.git("-C", str(planner), "rev-parse", "--is-shallow-repository"), "true")
        f.git("push", str(f.remote), newer + ":refs/heads/develop")
        bounded = DevelopHistory.from_head(newer, root=planner, runner=f.runner, depth=2)
        self.assertIn(f.source, bounded.positions)
        self.assertEqual(f.git("-C", str(planner), "rev-parse", "HEAD"), f.source)
        old = AllocationSnapshot(f.api, "fixture", f.work, AllocationIO(self.runner))
        self.assertFalse(old.load(event_source=f.source))
        self.assertIs(self.store.load(event_source=newer), self.store)

    def test_live_ref_oids_reuse_local_tags_and_fetch_only_missing_objects(self):
        f = self.fixture
        first = f.reservation()
        self.assertTrue(f.publisher.insert(first))
        latest = f.reservation(2)
        name = pending_name(latest)
        tag = (f"object {latest['source']}\ntype commit\ntag {name}\n"
               "tagger Fixture <fixture@example.invalid> 0 +0000\n\n")
        live = f.git("mktag", data=tag + canonical(latest) + "\n", remote=True)
        f.git("update-ref", "refs/tags/" + name, live, remote=True)
        stale = f.git("mktag", data=tag + canonical(dict(latest, run_id=3)) + "\n")
        f.git("update-ref", "refs/tags/" + name, stale)

        f.api.request = Mock(wraps=self.request)
        self.store.load()
        self.assertEqual(self.store.list("web"), [first, latest])
        tag_reads = [call.args[0] for call in f.api.request.call_args_list
                     if call.args[0].startswith("git/tags/")]
        self.assertEqual(tag_reads, ["git/tags/" + live])

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

    def test_overlapping_finalizers_need_a_lane_lock_failure_and_a_late_older_final_never_deploys(self):
        for component in LANES:
            with self.subTest(component=component):
                f, first, next_version = self.successor(component)
                old_receipt = f.receipt(first)
                new_receipt = f.receipt(next_version)
                runner = f.publisher.runner
                def finish_out_of_order(args, **kwargs):
                    # Only finalizers overlapping despite the lane lock can interleave like this.
                    if args[1] == "push" and args[-1].endswith(":refs/tags/" + old_receipt["tag"]):
                        self.store.commit()
                        self.assertTrue(f.publisher.finalize(new_receipt, lambda *_: True))
                    return runner(args, **kwargs)
                f.publisher.runner = finish_out_of_order
                # The late push lands, but cleanup sees the higher final and fails finalization.
                with self.assertRaisesRegex(Refusal, "superseded"):
                    f.publisher.finalize(old_receipt, lambda *_: True)
                tags = f.api.tags(component)
                self.assertCountEqual([tag["receipt"] for tag in tags], [old_receipt, new_receipt])
                self.assertEqual(f.publisher.list(), [first])
                f.api.require_current_versions({component: new_receipt})
                with self.assertRaisesRegex(Refusal, "superseded"):
                    f.api.require_current_versions({component: old_receipt})
                registry = Mock()
                registry.exists.return_value = True
                registry.manifest.return_value = (old_receipt["digest"], {})
                with self.assertRaises(ObsoleteSource):
                    verify(old_receipt, tags, registry, f.api.history(), automatic=True)

    def test_started_final_in_every_lane_survives_a_successor_allocation_and_keeps_its_cleanup(self):
        for component in LANES:
            for during_push in (True, False):
                with self.subTest(component=component, during_push=during_push):
                    f, first, successor = self.successor(component)
                    old_receipt = f.receipt(first)
                    runner = f.publisher.runner
                    def allocate_during_push(args, **kwargs):
                        if args[1] == "push" and args[-1].endswith(":refs/tags/" + old_receipt["tag"]):
                            self.store.commit()
                        return runner(args, **kwargs)
                    if during_push:
                        f.publisher.runner = allocate_during_push
                    self.assertTrue(f.publisher.finalize(old_receipt, lambda *_: True))
                    if not during_push:
                        # Allocation never contests the outcome, so it needs no replan.
                        self.store.commit()
                    self.assertEqual(self.store.list(), [first, successor])
                    self.assertEqual([tag["receipt"] for tag in f.api.tags(component)], [old_receipt])
                    self.assertEqual(f.publisher.list(component), [successor])
                    # Only the successor's final, never its pending reservation, withholds the old release.
                    f.api.require_current_versions({component: old_receipt})
                    f.publisher.runner = runner
                    self.assertTrue(f.publisher.finalize(f.receipt(successor), lambda *_: True))
                    self.assertEqual(f.publisher.list(component), [])
                    with self.assertRaisesRegex(Refusal, "superseded"):
                        f.api.require_current_versions({component: old_receipt})

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
