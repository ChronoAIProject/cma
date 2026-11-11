"""Every entry point checks authority and reviewed code before mutations."""

import contextlib
import http.cookiejar
import io
import json
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import identity
import oauth_journal
import review_gate
import runner
import session_journal
from safety import Refusal, write_private
from test_support import context, ordinary


class PhaseTests(unittest.TestCase):
    def test_sprint_security_cases_run_before_usage_and_domains(self):
        with context() as p:
            calls = []
            targets = ["baseline", "documents.run", "documents.updates", "revoked_replay", "resources.run",
                       "oauth_session.run", "journey.run", "usage.run", "domains.run"]
            with contextlib.ExitStack() as stack:
                stack.enter_context(patch("runner.identity.bind_live"))
                for target in targets:
                    stack.enter_context(patch("runner." + target,
                                              side_effect=lambda *a, name=target, **kw: calls.append(name)))
                runner.full_suite(p)
            self.assertEqual(calls, targets)

    def test_missing_privilege_evidence_fails_closed(self):
        for field in ("is_admin", "is_operator", "role", "is_active", "capabilities"):
            body = ordinary("test-a"); del body[field]
            self.assertFalse(identity.ordinary_profile(body, identity.fingerprint("test-a")))

    def test_extra_cma_capability_refuses_without_mutation(self):
        with context() as p:
            p.identities_bound = False; calls = []
            def request(origin, path, method="GET", token=None, **kwargs):
                calls.append(method); actor = token[-1]
                body = ordinary("test-" + actor) if path.endswith("/users/me") else {
                    "authenticated": True, "subject": "test-" + actor,
                    "capabilities": ["launch_triggers", "configure_platform"]}
                return 200, {}, json.dumps(body).encode(), 0
            p.http.request = request
            with self.assertRaises(Refusal): identity.bind_live(p)
            self.assertFalse(p.identities_bound)
            self.assertTrue(all(method == "GET" for method in calls))

    def test_review_go_receipt_must_match_source_and_manifest(self):
        with context():
            with self.assertRaises(Refusal): review_gate.require_go()
            write_private("review-go.json", {"verdict": "GO", "source_sha256": "other", "manifest_sha256": "other"})
            with self.assertRaises(Refusal): review_gate.require_go()

    def test_every_mutating_phase_checks_go_before_constructing_probe(self):
        with context():
            for phase in ("documents", "updates", "sessions", "usage", "domains", "journey",
                          "consent", "resources", "finish", "cleanup", "all"):
                with self.subTest(phase=phase), patch("runner.Probe") as factory:
                    with self.assertRaisesRegex(Refusal, "exact_source_security_review_go_required"):
                        runner.run_phase(SimpleNamespace(phase=phase, new_journey=False), None)
                    factory.assert_not_called()

    def test_failed_phase_or_interrupt_runs_cleanup_and_keeps_error(self):
        with context() as p, contextlib.redirect_stdout(io.StringIO()):
            class Output:
                def publish(self, value): self.value = value
            for failure in (Refusal("synthetic_probe_failure"), KeyboardInterrupt()):
                evidence = Output()
                with patch("runner.require_go"), patch("runner.Probe", return_value=p), \
                        patch("runner.identity.bind_live"), patch("runner.documents.run", side_effect=failure), \
                        patch("runner.cleanup.run") as cleanup:
                    code = runner.run_phase(SimpleNamespace(phase="documents", new_journey=False), evidence)
                self.assertEqual(code, 1); cleanup.assert_called_once_with(p)
                self.assertIsNotNone(evidence.value["stop_reason"])

    def test_unknown_callback_cannot_be_repeated_or_claimed_cleaned(self):
        with context() as p:
            oauth_journal.begin(p, "a"); oauth_journal.before_callback(p, "a")
            with self.assertRaisesRegex(Refusal, "requires_cleanup"): oauth_journal.begin(p, "a")
            with self.assertRaisesRegex(Refusal, "unknown_callback_cookie"): oauth_journal.cleaned(p, "a")
            self.assertEqual(session_journal.load(p)["oauth"]["a"]["status"], "callback_submitted")

    def test_cookie_receipt_is_durable_before_callback_returns(self):
        with context() as p:
            oauth_journal.begin(p, "a"); oauth_journal.before_callback(p, "a")
            cookie = http.cookiejar.Cookie(0, "cma_session", "synthetic-cookie", None, False,
                                           "cma.nyxid.ai", False, False, "/", True, True,
                                           None, True, None, None, {}, False)
            jar = oauth_journal.DurableCookies(p, "a"); jar.set_cookie(cookie)
            self.assertEqual(oauth_journal.known_cookie(p, "a"), "cma_session=synthetic-cookie")
            self.assertEqual(session_journal.load(p)["oauth"]["a"]["status"], "cookie_known")


if __name__ == "__main__":
    unittest.main()
