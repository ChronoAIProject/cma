"""Safety regressions for consent, quota, cleanup scope and private reporting."""

import contextlib
import datetime
import io
import json
import unittest
from unittest.mock import patch

import oauth_session
import resources
from journey_scope import allowed_write, reserve_launch
from probe import Probe
from safety import Refusal


class JourneyTests(unittest.TestCase):
    def context(self):
        p = object.__new__(Probe)
        p.accounts = {"a": {"subject": "private-a"}, "b": {"subject": "private-b"}}
        p.state = {"fixtures": {}, "writes": {}, "results": []}
        p.results = []
        p.save = lambda: None
        return p

    def test_reserved_launch_survives_restart_without_extra_charge(self):
        p = self.context()
        now = datetime.datetime(2026, 11, 11, 4, tzinfo=datetime.timezone.utc)
        reserve_launch(p, "a", now)
        reopened = self.context()
        reopened.state = json.loads(json.dumps(p.state))
        reserve_launch(reopened, "a", now)
        self.assertEqual(reopened.state["launch_daily_counts"], {"2026-11-11:a": 1})
        with self.assertRaises(Refusal):
            reserve_launch(reopened, "a", now + datetime.timedelta(days=1))

    def test_new_attempt_refuses_daily_budget_and_existing_sandbox(self):
        p = self.context()
        now = datetime.datetime(2026, 11, 11, 4, tzinfo=datetime.timezone.utc)
        p.state["launch_daily_counts"] = {"2026-11-11:a": 5}
        with self.assertRaises(Refusal):
            reserve_launch(p, "a", now)
        p.state["launch_daily_counts"].clear()
        p.state["fixtures"]["sandbox_a"] = {"id": "synthetic-sandbox"}
        with self.assertRaises(Refusal):
            reserve_launch(p, "a", now)

    @patch("journey_scope.read_private", return_value={"public_id": "approved-trigger"})
    def test_only_exact_approved_trigger_with_reservation_can_launch(self, _):
        p = self.context()
        route = "/api/v1/triggers/public/approved-trigger/launches"
        with self.assertRaises(Refusal):
            allowed_write(p, "a", route, "POST", {"start_if_ready": True})
        p.state["launch_reservations"] = {"a": {"day": "2026-11-11"}}
        self.assertTrue(allowed_write(p, "a", route, "POST", {"start_if_ready": True}))
        with self.assertRaises(Refusal):
            allowed_write(p, "a", route, "POST", {"start_if_ready": True, "choice": "new"})
        self.assertFalse(allowed_write(p, "a", route.replace("approved-trigger", "outside"), "POST", {}))
        self.assertFalse(allowed_write(p, "a", "/api/v1/triggers/approved-trigger", "PATCH", {}))

    @patch("journey_scope.read_private", return_value={})
    def test_cleanup_and_negative_writes_cannot_target_another_real_tenant(self, _):
        p = self.context()
        p.state["fixtures"]["sandbox_a"] = {"id": "synthetic-sandbox", "owner": "private-a"}
        route = "/api/v1/sandboxes/synthetic-sandbox"
        self.assertTrue(allowed_write(p, "a", route, "DELETE", None))
        self.assertTrue(allowed_write(p, "b", route, "DELETE", None))
        p.state["fixtures"]["sandbox_a"]["owner"] = "unrelated-owner"
        self.assertFalse(allowed_write(p, "a", route, "DELETE", None))
        self.assertFalse(allowed_write(p, "b", route, "DELETE", None))

    def test_report_never_contains_response_body_or_sensitive_headers(self):
        p = self.context()
        capture = io.StringIO()
        response = (200, {"Set-Cookie": "private-cookie"},
                    b'{"subject":"private-a","content":"private-content"}', 0.1)
        with contextlib.redirect_stdout(capture):
            p.record("owner_read", "PASS", [1549], response)
        rendered = capture.getvalue()
        for value in ("private-a", "private-content", "private-cookie"):
            self.assertNotIn(value, rendered)
        self.assertEqual(p.results[0]["response_bytes"], len(response[2]))
        self.assertEqual(len(p.results[0]["response_sha256"]), 64)

    def test_missing_visitor_resources_are_blocked_without_network_requests(self):
        p = self.context()
        p.request = lambda *args: self.fail("a missing fixture must not issue a probe")
        with contextlib.redirect_stdout(io.StringIO()):
            resources.run(p)
        self.assertEqual(len(p.results), 4)
        self.assertTrue(all(r["result"] == "BLOCKED" for r in p.results))

    def test_unexpected_oauth_scopes_never_reach_consent(self):
        p = self.context()
        p.login = lambda tenant: None
        calls = []
        class FakeHttp:
            def request(self, *args, **kwargs):
                calls.append(args)
                return 303, {"Location": "https://api.nyxid.ai/oauth/authorize?scope=unexpected"}, b"", 0
        p.http = FakeHttp()
        with self.assertRaises(Refusal):
            oauth_session.run(p)
        self.assertEqual(len(calls), 1)


if __name__ == "__main__":
    unittest.main()
