"""The allocator exposes a matrix only after every reservation is settled."""
import contextlib
import io
import json
import os
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

import staging_cd_allocate as allocation
from staging_cd_allocation_fixture import AllocationServer
from staging_cd_allocation_snapshot import AllocationConflict, AllocationSnapshot, SnapshotAPI
from staging_cd_allocation_io import AllocationIO
from staging_cd_history import DevelopHistory


class AllocationEntrypointTests(unittest.TestCase):
    def execute(self, *, fail_after_first=False):
        server = AllocationServer(0, components=("am", "web"))
        stores, output, events = [], io.StringIO(), []
        request = server.request

        def read(path, **kwargs):
            if path.startswith("git/ref/tags/"):
                events.append("readback")
            result = request(path, **kwargs)
            if fail_after_first and "/cma-web/pending/" in path:
                return dict(result, object={"type": "tag", "sha": "0" * 40})
            return result

        def store(api, token, root, bounded_io, *, mark):
            bounded_io.runner = server.command
            snapshot = AllocationSnapshot(api, token, root, bounded_io, mark=mark)
            stores.append(snapshot)
            return snapshot

        def emit(_env, key, value):
            events.append(key)
            if key == "matrix":
                self.assertEqual(len(json.loads(value)["include"]), 2)

        server.request = read
        env = {"GITHUB_EVENT_NAME": "push", "GITHUB_RUN_ID": "999", "GITHUB_RUN_ATTEMPT": "1",
               "GH_TOKEN": "fixture", "GITHUB_SHA": server.history().head, "CD_QUEUE": json.dumps(server.queue())}
        with patch.dict(os.environ, env, clear=True), contextlib.redirect_stdout(output), \
                patch.object(allocation, "event_identity"), patch.object(allocation, "GitHub", return_value=server), \
                patch.object(allocation, "Reservations", side_effect=store), \
                patch.object(allocation, "output", side_effect=emit) as emitted:
            status = allocation.main()
        return status, server, stores[0], output.getvalue(), events, emitted

    def test_success_emits_matrix_after_both_exact_ref_readbacks(self):
        status, server, store, output, events, emitted = self.execute()
        self.assertEqual(status, 0)
        self.assertEqual(events, ["readback", "readback", "matrix", "changed"])
        self.assertEqual(len(server.refs), 4)
        self.assertEqual(store.list(), store.planned)
        self.assertIn("phase=complete count=1", output)
        self.assertTrue(output.endswith("PASS\n"))
        self.assertEqual(emitted.call_count, 2)

    def test_partial_settlement_fails_without_output_or_repeating_prior_push(self):
        status, server, store, output, events, emitted = self.execute(fail_after_first=True)
        self.assertEqual(status, 1)
        self.assertEqual(events, ["readback", "readback"])
        emitted.assert_not_called()
        self.assertEqual(len(store.list()), 0)
        self.assertEqual(len(server.refs), 4)
        self.assertEqual(len([args for args in server.commands if args[1] == "push"]), 1)
        self.assertNotIn("phase=complete", output)
        self.assertIn("phase=readback-conflict count=1", output)
        self.assertTrue(output.endswith("FAIL\nFAIL allocation RESERVATION_VERIFY_FAILED\n"))
        self.assertNotIn("fixture", output)

    def test_replan_keeps_controller_proof_for_current_final(self):
        head = "a" * 40
        queue = {"head": head, "event_source": head, "changes": []}
        proofs = {"web": {"writer": head, "stamp": "web " + "a" * 64}}
        env = {"GH_TOKEN": "fixture", "GITHUB_RUN_ID": "999", "GITHUB_RUN_ATTEMPT": "1",
               "CD_CONTROLLER_PROOFS": json.dumps(proofs)}
        first, second = Mock(), Mock()
        first.commit.side_effect = AllocationConflict()
        with patch.object(allocation, "Reservations", side_effect=[first, second]), \
                patch.object(allocation, "GitHub"), patch.object(allocation, "allocate_queue",
                        return_value={"include": []}) as plan:
            self.assertEqual(allocation.allocate_current(queue, env, Mock(), Mock()), {"include": []})
        self.assertEqual([call.kwargs["proofs"] for call in plan.call_args_list], [proofs, proofs])
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
