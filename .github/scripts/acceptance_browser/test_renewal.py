import json
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "acceptance"))
from safety import Refusal, read_private
from test_support import context
import renew_sessions


class RenewalTests(unittest.TestCase):
    def test_foreign_account_refused_before_login(self):
        with context() as p:
            p.accounts["a"]["subject"] = "other-local-fixture"
            with self.assertRaises(Refusal):
                renew_sessions.renew(p, {"login_statuses": []})

    def test_interactive_verification_stops_before_second_actor(self):
        with context() as p:
            p.accounts["a"]["password"] = "local-fixture-only"
            calls = []

            def request(*args, **kwargs):
                calls.append(args[1])
                return 403, {}, json.dumps({"error_code": 2002}).encode(), 0

            p.http.request = request
            with self.assertRaisesRegex(Refusal, "interactive_mfa_required"):
                renew_sessions.renew(p, {"login_statuses": []})
            self.assertEqual(calls, ["/api/v1/auth/login"])

    def test_uncertain_login_is_not_replayed(self):
        with context() as p:
            p.accounts["a"]["password"] = "local-fixture-only"
            calls = []

            def request(*args, **kwargs):
                calls.append(args[1])
                raise Refusal("request_transport_failure")

            p.http.request = request
            with self.assertRaises(Refusal):
                renew_sessions.renew(p, {"login_statuses": []})
            with self.assertRaisesRegex(Refusal, "prior_login_requires_reconciliation"):
                renew_sessions.renew(p, {"login_statuses": []})
            self.assertEqual(len(calls), 1)

    def test_received_sessions_resume_without_new_logins(self):
        with context() as p, patch.object(renew_sessions, "bind_live"):
            for account in p.accounts.values():
                account["password"] = "local-fixture-only"
            calls = []

            def request(*args, **kwargs):
                actor = "a" if kwargs["body"]["email"].startswith("a@") else "b"
                calls.append(args[1])
                body = {
                    "user_id": p.accounts[actor]["subject"],
                    "access_token": "renewed-local-" + actor,
                    "refresh_token": "local-refresh-" + actor,
                }
                return 200, {}, json.dumps(body).encode(), 0

            p.http.request = request
            renew_sessions.renew(p, {"login_statuses": []})
            renew_sessions.renew(p, {"login_statuses": []})
            self.assertEqual(len(calls), 2)
            self.assertEqual(len(read_private("session-journal.json")["tokens"]), 2)


if __name__ == "__main__":
    unittest.main()
