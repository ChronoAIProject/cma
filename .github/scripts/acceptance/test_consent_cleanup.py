"""Self-scoped consent/cookie cleanup survives lost replies and journal writes."""

import contextlib
import io
import json
import unittest
from unittest.mock import patch

import oauth_journal
import session_journal
from safety import Refusal, write_private
from test_support import context


class ConsentCleanupTests(unittest.TestCase):
    def exercise(self, lost=None, point=None, after=False):
        with context() as p, contextlib.redirect_stdout(io.StringIO()):
            p.accounts["a"]["cma_cookie"] = "cma_session=synthetic-cookie"
            journal = session_journal.load(p)
            journal["oauth"] = {"a": {"status": "cookie_known", "cookie": p.accounts["a"]["cma_cookie"]}}
            write_private(session_journal.NAME, journal)
            cookie_live = True; consent_live = True; lost_once = False; calls = []; count = 0
            def request(origin, path, method="GET", **kwargs):
                nonlocal cookie_live, consent_live, lost_once
                calls.append((method, path))
                if path == "/auth/session":
                    body = {"authenticated": True, "subject": "test-a", "capabilities": ["launch_triggers"]}
                    return (200, {}, json.dumps(body).encode(), 0) if cookie_live else (401, {}, b'{}', 0)
                if path == "/auth/logout":
                    cookie_live = False
                    if lost == "cookie" and not lost_once:
                        lost_once = True; raise Refusal("synthetic_lost_cookie_reply")
                    return 303, {}, b'', 0
                if path == "/api/v1/users/me/consents":
                    return 200, {}, json.dumps({"consents": [{"client_id": "test-client"}] if consent_live else []}).encode(), 0
                if path == "/api/v1/users/me/consents/test-client" and method == "DELETE":
                    consent_live = False
                    if lost == "consent" and not lost_once:
                        lost_once = True; raise Refusal("synthetic_lost_consent_reply")
                    return 200, {}, b'{}', 0
                raise AssertionError("unexpected_cleanup_route")
            p.http.request = request
            real_save = p.save; real_write = oauth_journal.write_private
            def checkpoint(fn):
                nonlocal count
                count += 1
                if point == count and not after: raise Refusal("synthetic_before_persistence")
                fn()
                if point == count: raise Refusal("synthetic_after_persistence")
            p.save = lambda: checkpoint(real_save)
            with patch("oauth_journal.write_private", side_effect=lambda n, v: checkpoint(lambda: real_write(n, v))):
                try: session_journal.revoke_cma(p, "a")
                except Refusal: pass
            p.save = real_save; session_journal.revoke_cma(p, "a")
            self.assertFalse(cookie_live); self.assertFalse(consent_live)
            self.assertEqual(sum(path == "/auth/logout" for _, path in calls), 1)
            self.assertEqual(sum(method == "DELETE" for method, _ in calls), 1)
            self.assertEqual(session_journal.load(p)["oauth"]["a"]["status"], "cleaned")
            return count

    def test_lost_cookie_logout_and_consent_delete_reconcile_absence(self):
        for lost in ("cookie", "consent"):
            with self.subTest(lost=lost): self.exercise(lost=lost)

    def test_each_consent_cleanup_persistence_boundary_resumes(self):
        count = self.exercise()
        for point in range(1, count + 1):
            for after in (False, True):
                with self.subTest(point=point, after=after): self.exercise(point=point, after=after)

    def test_foreign_cookie_refuses_logout_and_consent_write(self):
        with context() as p:
            p.accounts["a"]["cma_cookie"] = "cma_session=synthetic-cookie"
            calls = []
            def request(origin, path, method="GET", **kwargs):
                calls.append((method, path))
                return 200, {}, b'{"authenticated":true,"subject":"unrelated","capabilities":["launch_triggers"]}', 0
            p.http.request = request
            with self.assertRaisesRegex(Refusal, "cookie_identity_mismatch"):
                session_journal.revoke_cma(p, "a")
            self.assertEqual(calls, [("GET", "/auth/session")])


if __name__ == "__main__":
    unittest.main()
