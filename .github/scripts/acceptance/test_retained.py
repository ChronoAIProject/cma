"""Retained fixture caps, grant interruption and output-independent cleanup."""

import contextlib
import io
import json
import unittest
from unittest.mock import patch

import cleanup
import grant_cleanup
import retained
from safety import Refusal
from test_support import context, restart


def seed_document(p):
    body = {"title": "CMA acceptance fixture", "markdown": "Synthetic."}
    intent = {"tenant": "a", "path": "/api/v1/documents", "method": "POST", "body": body}
    retained.reserve(p, "a", "create", intent)
    doc = {"id": "test-document", "owner_subject": "test-a", "revision": 1}
    retained.observed(p, "a", "create", intent, 201, doc)
    p.fixture("document_a", doc)
    return intent


class RetainedTests(unittest.TestCase):
    def test_inflight_grant_cannot_be_cleared_by_an_early_denial(self):
        with context() as p:
            seed_document(p)
            path = "/api/v1/documents/test-document/shares/test-b"
            body = {"expected_revision": 1, "permission": "read"}
            p.state["writes"]["grant"] = {"key": "test-grant", "status": "submitted",
                                            "intent": {"tenant": "a", "path": path, "method": "PUT", "body": body}}
            grant_cleanup.before_grant(p, "a", path, body, "test-grant")
            calls = []
            def request(origin, route, method="GET", **kwargs):
                calls.append(method)
                if method == "PUT":
                    return 409, {}, b'{"code":"idempotency_in_flight"}', 0
                return 404, {}, b'{"code":"document_not_found"}', 0
            p.http.request = request
            with self.assertRaisesRegex(Refusal, "original_receipt_still_uncertain"):
                grant_cleanup.run(p)
            self.assertEqual(calls, ["PUT"])
            self.assertIn("a", grant_cleanup.load(p)["pending"])

    def test_cap_survives_campaign_replacement_and_ledger_is_hash_only(self):
        with context() as p:
            original = seed_document(p)
            p.state["campaign"] = "new-campaign"
            with self.assertRaisesRegex(Refusal, "retained_fixture_cap"):
                retained.reserve(p, "a", "new-key", original)
            retained.reserve(p, "a", "create", original)
            rendered = json.dumps(retained.load(p))
            for value in ("test-a", "test-document", "CMA acceptance fixture"):
                self.assertNotIn(value, rendered)

    def test_uncertain_create_never_replays_a_permanent_slot(self):
        with context() as p:
            calls = []
            def request(origin, path, method="GET", **kwargs):
                calls.append(method)
                raise Refusal("synthetic_lost_response")
            p.http.request = request
            body = {"title": "CMA acceptance fixture", "markdown": "Synthetic."}
            with self.assertRaises(Refusal):
                p.write("document_a_create", "a", "/api/v1/documents", "POST", body)
            restart(p)
            before = retained.load(p)
            with self.assertRaisesRegex(Refusal, "consumed_document_slot"):
                retained.reconcile(p)
            self.assertEqual(calls, ["POST"])
            self.assertEqual(retained.load(p), before)

    def test_direct_create_and_unowned_mutations_refuse_before_http(self):
        with context() as p:
            seed_document(p)
            for path, method, body in (
                ("/api/v1/documents", "POST", {"title": "unreserved"}),
                ("/api/v1/documents/another", "PATCH", {}),
                ("/api/v1/documents/test-document/shares/another", "PUT", {}),
                ("/api/v1/documents/test-document", "PATCH", {"expected_revision": 1}),
                ("/api/v1/admin/settings", "PATCH", {}),
                ("/api/v1/agents", "POST", {}),
            ):
                with self.subTest(path=path), self.assertRaises(Refusal):
                    p.request("a", path, method, body, key="unreserved")

    def exercise_grant(self, point=None, after=False, lost_delete=False):
        with context() as p, contextlib.redirect_stdout(io.StringIO()):
            seed_document(p)
            path = "/api/v1/documents/test-document/shares/test-b"
            body = {"expected_revision": 1, "permission": "read"}
            p.state["writes"]["grant"] = {"key": "test-grant", "status": "submitted",
                                            "intent": {"tenant": "a", "path": path, "method": "PUT", "body": body}}
            p.save()
            grant_cleanup.before_grant(p, "a", path, body, "test-grant")
            live = True; deletes = 0; count = 0; lost = False
            def request(origin, path, method="GET", **kwargs):
                nonlocal live, deletes, lost
                if path.endswith("/source"):
                    return (200, {}, b'{}', 0) if live else (404, {}, b'{"code":"document_not_found"}', 0)
                if method == "DELETE":
                    deletes += 1; live = False
                    if lost_delete and not lost:
                        lost = True; raise Refusal("lost_cleanup_reply")
                    return 200, {}, b'{"id":"test-document","owner_subject":"test-a","revision":2}', 0
                return 200, {}, b'{"id":"test-document","owner_subject":"test-a","revision":1}', 0
            p.http.request = request
            real_save = p.save; real_write = grant_cleanup.write_private
            def checkpoint(fn):
                nonlocal count
                count += 1
                if count == point and not after: raise Refusal("synthetic_before_save")
                fn()
                if count == point: raise Refusal("synthetic_after_save")
            p.save = lambda: checkpoint(real_save)
            with patch("grant_cleanup.write_private", side_effect=lambda n, v: checkpoint(lambda: real_write(n, v))):
                try: grant_cleanup.run(p)
                except Refusal: pass
            p.save = real_save; restart(p); grant_cleanup.run(p)
            self.assertFalse(live); self.assertEqual(deletes, 1)
            self.assertEqual(grant_cleanup.load(p)["pending"], {})
            return count

    def test_grant_cleanup_resumes_every_persistence_and_lost_delete(self):
        count = self.exercise_grant()
        for point in range(1, count + 1):
            for after in (False, True):
                with self.subTest(point=point, after=after): self.exercise_grant(point, after)
        self.exercise_grant(lost_delete=True)

    def test_failed_probe_still_attempts_independent_resource_cleanup(self):
        with context() as p:
            with patch("cleanup.retained.reconcile", side_effect=Refusal("pending")), \
                    patch("cleanup.grant_cleanup.run") as grants, \
                    patch("cleanup.launch_recovery.cleanup") as launches:
                with self.assertRaisesRegex(Refusal, "resource_cleanup_pending"): cleanup.resources(p)
                grants.assert_called_once_with(p)
                self.assertEqual(launches.call_count, 2)

    def test_report_expiry_is_verified_without_new_report(self):
        with context() as p, contextlib.redirect_stdout(io.StringIO()):
            intent = {"tenant": "a", "path": "/api/v1/usage/metrics/reports", "method": "POST", "body": {}}
            retained.reserve(p, "a", "report-key", intent)
            retained.observed(p, "a", "report-key", intent, 202, {"report_id": "test-report", "expires_at": 1})
            p.fixture("report_a", {"report_id": "test-report"})
            calls = []
            def request(origin, path, method="GET", **kwargs):
                calls.append(method); return 404, {}, b'{"code":"not_found"}', 0
            p.http.request = request; retained.verify_expiration(p)
            self.assertEqual(calls, ["GET"])
            self.assertTrue(retained.load(p)["slots"]["a:report"]["expiry_verified"])
            with self.assertRaises(Refusal): retained.reserve(p, "a", "replacement", intent)


if __name__ == "__main__":
    unittest.main()
