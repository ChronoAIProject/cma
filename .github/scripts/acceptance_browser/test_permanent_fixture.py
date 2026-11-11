import json
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "acceptance"))
from safety import Refusal, read_private, write_private
from test_support import context
import permanent_fixture as fixture


class PermanentFixtureTests(unittest.TestCase):
    def test_existing_resource_or_incomplete_inventory_prevents_reservation(self):
        for body in (
            {"items": [{"id": "local-fixture"}]},
            {"items": [], "next_cursor": "more"},
            {},
        ):
            with self.subTest(body=body), context() as p:
                p.request = lambda *_: (200, {}, json.dumps(body).encode(), 0)
                with self.assertRaises(Refusal):
                    fixture.prepare(p)
                self.assertIsNone(read_private(fixture.NAME))

    def test_original_create_is_submitted_once_and_never_replaced(self):
        with context() as p:
            p.request = lambda *_: (200, {}, b'{"items":[]}', 0)
            entry = fixture.prepare(p)
            calls = []

            def request(*args, **kwargs):
                calls.append((args[1], kwargs["headers"]["Idempotency-Key"]))
                return 202, {}, b'{"id":"local-operation","status":"queued"}', 0

            p.http.request = request
            fixture.admit(p, entry)
            fixture.admit(p, fixture.prepare(p))
            self.assertEqual(len(calls), 1)
            self.assertEqual(read_private(fixture.NAME)["key"], calls[0][1])

    def test_uncertain_submission_and_consumed_absence_never_mutate(self):
        with context() as p:
            p.request = lambda *_: (200, {}, b'{"items":[]}', 0)
            entry = fixture.prepare(p)
            entry["status"] = "submitted"
            write_private(fixture.NAME, entry)
            with self.assertRaisesRegex(Refusal, "reconciliation_required"):
                fixture.admit(p, fixture.prepare(p))

    def test_absent_parent_retains_idle_obligation_without_mutation(self):
        with context() as p:
            p.request = lambda *_: (200, {}, b'{"items":[]}', 0)
            entry = fixture.prepare(p)
            result = fixture.idle(p, entry)
            self.assertEqual(result["status"], "NO_PARENT")
            self.assertTrue(read_private(fixture.NAME)["idle_required"])

    def test_foreign_parent_cannot_be_paused(self):
        with context() as p:
            p.request = lambda *_: (200, {}, b'{"items":[]}', 0)
            entry = fixture.prepare(p)
            entry["sandbox_id"] = "local-parent"
            p.request = lambda *_: (
                200,
                {},
                b'{"sandbox":{"owner":"another-actor","status":"active"}}',
                0,
            )
            with self.assertRaisesRegex(Refusal, "idle_owner_mismatch"):
                fixture.idle(p, entry)
            self.assertTrue(read_private(fixture.NAME)["idle_required"])

    def test_pause_receipt_is_not_idle_proof_and_reuses_original_key(self):
        with context() as p:
            p.request = lambda *_: (200, {}, b'{"items":[]}', 0)
            entry = fixture.prepare(p)
            entry["sandbox_id"] = "local-parent"
            state = {"owner": p.accounts["a"]["subject"], "status": "active"}
            p.request = lambda *_: (200, {}, json.dumps({"sandbox": state}).encode(), 0)
            calls = []

            def pause(*args, **kwargs):
                calls.append(kwargs["headers"]["Idempotency-Key"])
                self.assertTrue(read_private(fixture.NAME)["idle_required"])
                return 202, {}, b"{}", 0

            p.http.request = pause
            self.assertEqual(fixture.idle(p, entry)["status"], "PAUSE_PENDING")
            self.assertTrue(read_private(fixture.NAME)["idle_required"])
            fixture.idle(p, fixture.load(p))
            self.assertEqual(calls[0], calls[1])
            state["status"] = "paused"
            self.assertEqual(fixture.idle(p, fixture.load(p))["status"], "PAUSED")
            self.assertFalse(read_private(fixture.NAME)["idle_required"])
            self.assertEqual(len(calls), 2)


if __name__ == "__main__":
    unittest.main()
