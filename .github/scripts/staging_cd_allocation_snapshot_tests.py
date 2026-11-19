"""Single-snapshot allocation preserves authority with exact-ref settlement."""
import json
from pathlib import Path
import subprocess
import unittest
from unittest.mock import Mock

from staging_cd_allocate import allocate_queue
from staging_cd_allocation_fixture import AllocationServer
from staging_cd_allocation_io import AllocationIO
from staging_cd_allocation_snapshot import AllocationSnapshot, SnapshotAPI
from staging_cd_common import Refusal
from staging_cd_github import GitHubNotFound
from staging_cd_receipts_tests import fixture_receipt
from staging_cd_receipts import canonical
from staging_cd_versions import DEPLOYABLES


class AllocationSnapshotTests(unittest.TestCase):
    def fixture(self, count=3, **kwargs):
        server = AllocationServer(count, **kwargs)
        store = AllocationSnapshot(server, "fixture", Path.cwd(), AllocationIO(server.command))
        return server, store

    def allocate(self, server, store):
        store.load()
        matrix = allocate_queue(server.queue(), SnapshotAPI(server, store), store, 999, 1)
        store.commit()
        return matrix

    def test_one_snapshot_one_push_and_only_unfenced_owner_read(self):
        server, store = self.fixture(25, latency=0.001)
        matrix = self.allocate(server, store)
        tasks = json.loads(matrix["include"][0]["tasks"])
        self.assertEqual(len(tasks), 1)
        self.assertEqual(tasks[0]["reservation"]["version"], "0.10.26")
        self.assertEqual([args[1] for args in server.commands].count("ls-remote"), 1)
        pushes = [args for args in server.commands if args[1] == "push"]
        self.assertEqual(len(pushes), 1)
        self.assertEqual(len([arg for arg in pushes[0] if ":refs/tags/" in arg]), 1)
        self.assertEqual([path for path in server.calls if path.startswith("actions/")],
                         ["actions/runs/34/attempts/1"])
        self.assertFalse(any("matching-refs" in path for path in server.calls))
        self.assertGreater(server.peak, 1)
        self.assertLessEqual(server.peak, 8)
        self.assertTrue(all(server.refs[name] == oid for name, oid in server.original.items()))
        with self.assertRaises(Refusal):
            store.load()

    def test_unreserved_bumps_settle_once_each_after_all_provenance_is_verified(self):
        server, store = self.fixture(3, reserved=False)
        self.allocate(server, store)
        self.assertEqual([value["version"] for value in store.planned], ["0.10.1", "0.10.2", "0.10.3", "0.10.4"])
        self.assertEqual(len([args for args in server.commands if args[1] == "push"]), 4)
        self.assertEqual(len([path for path in server.calls if path.startswith("git/ref/tags/")]), 4)

    def test_live_unfenced_owner_refuses_and_old_owner_reads_are_forbidden(self):
        server, store = self.fixture()
        request = server.request
        def guarded(path, **kwargs):
            if path.startswith("actions/"):
                self.assertEqual(path, "actions/runs/12/attempts/1")
                return dict(request(path, **kwargs), status="in_progress")
            return request(path, **kwargs)
        server.request = guarded
        with self.assertRaisesRegex(Refusal, "drained"):
            self.allocate(server, store)
        self.assertFalse(any(args[1] == "push" for args in server.commands))

    def test_inventory_refusal_has_no_write_or_partial_success(self):
        for output in ("invalid", "a" * 40 + "\trefs/tags/cma-agent-manager/pending/vinvalid\n",
                       ("a" * 40 + "\trefs/tags/fixture\n") * 2):
            server, store = self.fixture()
            store.io.runner = Mock(return_value=subprocess.CompletedProcess([], 0, output, ""))
            with self.assertRaises(Refusal):
                store.load()
            self.assertFalse(store.loaded)
            self.assertEqual(store.planned, [])

    def test_annotation_binding_and_partial_reads_fail_closed(self):
        for field, value in (("sha", "0" * 40), ("tag", "wrong"), ("object", {"type": "commit", "sha": "e" * 40}),
                             ("message", "{}")):
            server, store = self.fixture()
            server.objects[next(iter(server.objects))][field] = value
            with self.assertRaises(Refusal):
                store.load()
            self.assertFalse(store.loaded)
            self.assertFalse(any(args[1] == "push" for args in server.commands))

    def test_final_receipt_is_bound_to_snapshot_oid_and_history(self):
        server, store = self.fixture()
        receipt = fixture_receipt(source=server.sources[-1])
        oid = "f" * 40
        server.refs[receipt["tag"]] = oid
        server.objects[oid] = {"sha": oid, "tag": receipt["tag"], "message": canonical(receipt),
                               "object": {"type": "commit", "sha": receipt["source"]}}
        store.load()
        self.assertEqual(store.finals[0]["receipt"], receipt)
        server.objects[oid]["object"]["sha"] = server.sources[0]
        replacement = AllocationSnapshot(server, "fixture", Path.cwd(), AllocationIO(server.command))
        with self.assertRaisesRegex(Refusal, "receipt"):
            replacement.load()

    def test_collision_and_mismatched_readback_never_retry_push(self):
        for mode in ("collision", "corrupt_readback"):
            server, store = self.fixture()
            setattr(server, mode, True)
            with self.assertRaises(Refusal):
                self.allocate(server, store)
            self.assertEqual(len([args for args in server.commands if args[1] == "push"]), 1)
            self.assertTrue(all(server.refs[name] == oid for name, oid in server.original.items()))

    def test_ambiguous_response_requires_exact_ref_readback_after_each_push(self):
        server, store = self.fixture(3, reserved=False)
        server.ambiguous = True
        self.allocate(server, store)
        self.assertEqual(len(server.refs), 4)
        self.assertEqual(len([args for args in server.commands if args[1] == "push"]), 4)
        self.assertEqual(store.list(), store.planned)

    def test_missing_readbacks_retry_only_the_exact_ref_then_settle_or_refuse(self):
        for misses in (2, 3):
            server, store = self.fixture(0)
            request, marks = server.request, []
            store.progress = lambda *args: marks.append(args)
            store.io.sleep = Mock()
            reads = []
            def delayed(path, **kwargs):
                if path.startswith("git/ref/tags/"):
                    reads.append(path)
                    if len(reads) <= misses:
                        raise GitHubNotFound("GitHub resource was not found")
                return request(path, **kwargs)
            server.request = delayed
            if misses == 2:
                self.allocate(server, store)
                self.assertEqual(store.list(), store.planned)
                self.assertIn(("am", "reservation-readback"), marks)
            else:
                with self.assertRaisesRegex(Refusal, "unavailable"):
                    self.allocate(server, store)
                self.assertEqual(store.list(), [])
                self.assertEqual(len(server.refs), 1)  # Remote version remains burned.
            self.assertEqual(len(reads), 3)
            self.assertEqual(len(set(reads)), 1)
            self.assertEqual(store.io.sleep.call_count, 2)
            self.assertEqual(marks.count(("am", "readback-missing")), misses)
            self.assertEqual(len([args for args in server.commands if args[1] == "push"]), 1)
            self.assertEqual(len([args for args in server.commands if args[1] == "ls-remote"]), 1)
            with self.assertRaisesRegex(Refusal, "already attempted"):
                store.commit()

    def test_conflicting_or_unreadable_ref_refuses_without_visibility_retries(self):
        for response in (Refusal("private transport"), {},
                         {"ref": "refs/tags/cma-agent-manager/pending/v0.10.1",
                          "object": {"type": "commit", "sha": "a" * 40}}):
            server, store = self.fixture(0)
            request, reads, marks = server.request, [], []
            store.progress = lambda *args: marks.append(args)
            store.io.sleep = Mock()
            def invalid(path, **kwargs):
                if path.startswith("git/ref/tags/"):
                    reads.append(path)
                    if isinstance(response, Exception):
                        raise response
                    return response
                return request(path, **kwargs)
            server.request = invalid
            with self.assertRaises(Refusal):
                self.allocate(server, store)
            self.assertEqual(len(reads), 1)
            store.io.sleep.assert_not_called()
            self.assertEqual(store.list(), [])
            if not isinstance(response, Exception):
                self.assertIn(("am", "readback-conflict"), marks)

    def test_five_components_keep_one_inventory_and_update_only_verified_memory(self):
        server, store = self.fixture(4, components=tuple(DEPLOYABLES))
        matrix = self.allocate(server, store)
        self.assertEqual(len(matrix["include"]), 5)
        self.assertEqual(len(store.list()), 25)
        self.assertEqual(len([args for args in server.commands if args[1] == "push"]), 5)
        self.assertEqual(len([args for args in server.commands if args[1] == "ls-remote"]), 1)
        self.assertEqual(len([path for path in server.calls if path.startswith("git/ref/tags/")]), 5)
        self.assertEqual(len([path for path in server.calls if path.startswith("actions/")]), 1)
        for group in matrix["include"]:
            self.assertEqual(json.loads(group["tasks"])[0]["reservation"]["version"], "0.10.5")

    def test_unknown_label_prevents_the_entire_batch(self):
        server, store = self.fixture(3, reserved=False)
        request = server.request
        def missing(path, **kwargs):
            result = request(path, **kwargs)
            return dict(result, labels=[]) if path == "pulls/3" else result
        server.request = missing
        with self.assertRaisesRegex(Refusal, "version-upgrade"):
            self.allocate(server, store)
        self.assertEqual(store.planned, [])
        self.assertEqual(server.refs, {})

    def test_noop_never_pushes_and_snapshot_is_not_usable_before_load(self):
        server, store = self.fixture()
        with self.assertRaises(Refusal):
            store.list()
        with self.assertRaises(Refusal):
            SnapshotAPI(server, store).tags()
        with self.assertRaises(Refusal):
            store.commit()
        store.load()
        store.commit()
        self.assertFalse(any(args[1] == "push" for args in server.commands))
