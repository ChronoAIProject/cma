"""The allocator exposes a matrix only after every reservation is settled."""
import contextlib
import io
import json
import os
import unittest
from unittest.mock import patch

import staging_cd_allocate as allocation
from staging_cd_allocation_fixture import AllocationServer
from staging_cd_allocation_snapshot import AllocationSnapshot


class AllocationEntrypointTests(unittest.TestCase):
    def execute(self, *, fail_after_first=False):
        server = AllocationServer(0, components=("am", "web"))
        stores, output, events = [], io.StringIO(), []
        request = server.request

        def read(path, **kwargs):
            if path.startswith("git/ref/tags/"):
                events.append("readback")
            result = request(path, **kwargs)
            if fail_after_first and events.count("readback") == 2:
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
               "GH_TOKEN": "fixture", "CD_QUEUE": json.dumps(server.queue())}
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
        self.assertEqual(len(server.refs), 2)
        self.assertEqual(store.list(), store.planned)
        self.assertIn("phase=complete count=1", output)
        self.assertTrue(output.endswith("PASS\n"))
        self.assertEqual(emitted.call_count, 2)

    def test_partial_settlement_fails_without_output_or_repeating_prior_push(self):
        status, server, store, output, events, emitted = self.execute(fail_after_first=True)
        self.assertEqual(status, 1)
        self.assertEqual(events, ["readback", "readback"])
        emitted.assert_not_called()
        self.assertEqual(len(store.list()), 1)
        self.assertEqual(len(server.refs), 2)
        self.assertEqual(len([args for args in server.commands if args[1] == "push"]), 2)
        self.assertNotIn("phase=complete", output)
        self.assertIn("phase=readback-conflict count=1", output)
        self.assertTrue(output.endswith("FAIL\nFAIL allocation RESERVATION_VERIFY_FAILED\n"))
        self.assertNotIn("fixture", output)
