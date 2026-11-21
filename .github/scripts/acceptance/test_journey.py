"""Policy, quota and restart fences use independent durable history."""

import contextlib
import datetime
import io
import unittest
from unittest.mock import patch

import launch_ledger as ledger
import launch_recovery
import oauth_session
import resources
import trigger_pin
from journey_scope import allowed_write
from safety import Refusal, read_private, write_private
from test_support import context, intent, launch_value, restart

NOW = datetime.datetime(2026, 11, 11, 4, tzinfo=datetime.timezone.utc)


class JourneyTests(unittest.TestCase):
    def test_sixth_intent_refused_across_restart_and_new_campaign(self):
        with context() as p:
            for index in range(5):
                key = "synthetic-" + str(index)
                ledger.reserve(p, "a", key, intent(), NOW)
                ledger.observed(p, key, 409, {"code": "sandbox_credential_setup_required"})
                restart(p); p.state["campaign"] = "replacement-" + str(index)
            with self.assertRaisesRegex(Refusal, "visitor_daily_launch_budget"):
                ledger.reserve(p, "a", "sixth", intent(), NOW)
            self.assertEqual(len(ledger.load(p)["entries"]), 5)
            ledger.reserve(p, "a", "next-day", intent(), NOW + datetime.timedelta(days=1))

    def test_original_reservation_survives_restart_and_pending_blocks_rollover(self):
        with context() as p:
            ledger.reserve(p, "a", "original", intent(), NOW); restart(p)
            ledger.reserve(p, "a", "original", intent(), NOW)
            self.assertEqual(len(ledger.load(p)["entries"]), 1)
            with self.assertRaisesRegex(Refusal, "pending_launch"):
                ledger.reserve(p, "a", "replacement", intent(), NOW + datetime.timedelta(days=1))
            with self.assertRaisesRegex(Refusal, "outside_safe_receipt_window"):
                ledger.validate(p, "a", "original", intent(), NOW + datetime.timedelta(hours=23))

    def test_missing_or_uncertain_history_refuses(self):
        with context() as p:
            from safety import private_path
            for value in (None, {"version": 1, "complete_history": False}):
                private_path(ledger.NAME).unlink(missing_ok=True)
                if value is not None: write_private(ledger.NAME, value)
                with self.assertRaises(Refusal): ledger.reserve(p, "a", "new", intent(), NOW)

    def test_exact_actor_key_body_and_nonstarting_policy_required(self):
        with context() as p, patch("launch_ledger.utc_now", return_value=NOW):
            draft = intent(); ledger.reserve(p, "a", "original", draft)
            self.assertTrue(allowed_write(p, "a", draft["path"], "POST", draft["body"], "original"))
            for actor, key, body in (("b", "original", draft["body"]), ("a", "new", draft["body"]),
                                     ("a", "original", {"start_if_ready": True})):
                with self.assertRaises(Refusal): allowed_write(p, actor, draft["path"], "POST", body, key)
            with self.assertRaises(Refusal):
                ledger.reserve(p, "b", "start", dict(intent("b"), body={"start_if_ready": True}))

    def test_wrong_trigger_descriptor_policy_and_revision_refuse(self):
        with context() as p:
            original = read_private("acceptance-trigger.json")
            for field, value in (("public_id", "another-trigger"), ("public_descriptor", {})):
                write_private("acceptance-trigger.json", dict(original, **{field: value}))
                with self.assertRaises(Refusal): trigger_pin.configured(p)
            write_private("acceptance-trigger.json", original)
            for field, value in (("revision_digest", "new-revision"), ("public_id", "another-trigger"),
                                 ("trust", {"disclosures": ["unexpected"]})):
                bad = dict(launch_value(), **{field: value})
                p.state["fixtures"]["launch_a"] = bad
                with self.assertRaises(Refusal):
                    allowed_write(p, "a", "/api/v1/launches/test-launch/start", "POST", {"acknowledgements": []})

    def test_second_live_sandbox_is_refused(self):
        import json
        with context() as p, contextlib.redirect_stdout(io.StringIO()):
            p.state["fixtures"]["sandbox_a"] = {"id": "first", "owner": "test-a"}
            p.http.request = lambda *a, **kw: (200, {}, json.dumps({"id": "test-sandbox", "owner": "test-a"}).encode(), 0)
            with self.assertRaisesRegex(Refusal, "second_live"):
                launch_recovery.remember(p, "a", launch_value(True))

    def test_unknown_inflight_or_recovery_errors_keep_reservation(self):
        with context() as p:
            ledger.reserve(p, "a", "original", intent(), NOW)
            for code in ("idempotency_in_flight", "idempotency_recovery_required", "idempotency_key_reused"):
                ledger.observed(p, "original", 409, {"code": code})
                self.assertEqual(ledger.load(p)["entries"]["original"]["status"], "submitted")

    def test_missing_resources_are_explicitly_blocked(self):
        with context() as p, contextlib.redirect_stdout(io.StringIO()):
            resources.run(p)
            self.assertEqual(len(p.results), 4)
            self.assertTrue(all(r["result"] == "BLOCKED" for r in p.results))

    def test_unexpected_oauth_scope_never_reaches_consent(self):
        with context() as p:
            calls = []
            def request(*args, **kwargs):
                calls.append(args)
                return 303, {"Location": "https://api.nyxid.ai/oauth/authorize?scope=unexpected"}, b"", 0
            p.http.request = request
            with self.assertRaises(Refusal): oauth_session.run(p)
            self.assertEqual(len(calls), 1)


if __name__ == "__main__":
    unittest.main()
