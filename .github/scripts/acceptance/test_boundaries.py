"""Unsafe output and substituted identities must fail before mutations."""

import contextlib
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from evidence import Evidence
import identity
from probe import Probe
from safety import Http, Refusal
import runner
from test_support import ordinary


class BoundaryTests(unittest.TestCase):
    def test_arbitrary_problem_codes_and_body_values_cannot_escape(self):
        p = object.__new__(Probe)
        p.results = []
        p.state = {"results": []}
        p.save = lambda: None
        for raw in ("private_subject", "private_access_value", "test@example.invalid", "unexpected_server_text"):
            payload = json.dumps({"code": raw, "subject": raw, "email": raw, "access_token": raw}).encode()
            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                p.record("synthetic_denial", "PASS", [1549], (401, {"Set-Cookie": raw}, payload, 0))
            self.assertNotIn(raw, output.getvalue())
            self.assertNotIn(raw, json.dumps(p.results[-1]))
            self.assertEqual(p.results[-1]["problem_code"], "unrecognized")

    def test_evidence_refusals_precede_probe_construction(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            broad = root / "broad"; broad.mkdir(mode=0o755)
            closed = root / "closed"; closed.mkdir(mode=0o500)
            good = root / "private"; good.mkdir(mode=0o700)
            link = root / "link"; link.symlink_to(good, target_is_directory=True)
            existing = good / "existing.json"; existing.write_text("original")
            repo = root / "repository"; repo.mkdir(mode=0o700); (repo / ".git").mkdir()
            for destination in (broad / "out", closed / "out", link / "out", existing,
                                root / "missing" / "out", repo / "out"):
                with patch("sys.argv", ["runner", "baseline", "--output", str(destination)]), \
                        patch("runner.Probe") as factory, patch("safety.Http.request") as request:
                    with self.assertRaises(Refusal):
                        runner.main()
                    factory.assert_not_called()
                    request.assert_not_called()
            closed.chmod(0o700)
            self.assertEqual(existing.read_text(), "original")

    def test_output_reserved_before_work_and_published_privately(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve(); target = root / "result.json"
            with Evidence(target) as evidence:
                self.assertEqual(json.loads(target.read_text())["status"], "reserved_incomplete")
                evidence.publish({"result": "PASS"})
            self.assertEqual(json.loads(target.read_text()), {"result": "PASS"})
            self.assertEqual(target.stat().st_mode & 0o777, 0o600)
            self.assertEqual(list(root.iterdir()), [target])

    def test_failed_phase_leaves_fixed_incomplete_artifact(self):
        with tempfile.TemporaryDirectory() as temporary:
            target = Path(temporary).resolve() / "result.json"
            with self.assertRaises(RuntimeError):
                with Evidence(target):
                    raise RuntimeError("sensitive_exception_value")
            self.assertNotIn("sensitive_exception_value", target.read_text())
            self.assertEqual(json.loads(target.read_text())["status"], "incomplete")

    def actor_context(self):
        p = object.__new__(Probe)
        p.accounts = {a: {"subject": "test-" + a, "email": a + "@example.invalid", "access_token": a}
                      for a in ("a", "b")}
        p.approval = {"actors": {a: {"subject_sha256": identity.fingerprint(v["subject"]),
                                     "email_sha256": identity.fingerprint(v["email"])}
                                  for a, v in p.accounts.items()}, "capabilities": ["launch_triggers"]}
        p.identities_bound = False; calls = []
        class FakeHttp:
            writes_authorized = False
            def request(self, origin, path, method="GET", token=None, **kwargs):
                calls.append((method, path))
                body = ordinary("test-" + token) if path.endswith("/users/me") else {
                    "authenticated": True, "subject": "test-" + token, "capabilities": ["launch_triggers"]}
                return 200, {}, json.dumps(body).encode(), 0
        p.http = FakeHttp()
        return p, calls

    def test_substituted_valid_bearer_never_mutates_or_revokes(self):
        p, calls = self.actor_context()
        p.accounts["b"]["access_token"] = "unrelated-valid-actor"
        with self.assertRaises(Refusal): identity.bind_live(p)
        self.assertFalse(p.identities_bound)
        self.assertTrue(all(method == "GET" for method, _ in calls))

    def test_editable_subject_cannot_replace_reviewed_pin(self):
        p, calls = self.actor_context()
        p.accounts["b"].update(subject="unrelated", email="unrelated@example.invalid")
        with self.assertRaises(Refusal): identity.bind_live(p)
        self.assertEqual(calls, [])

    def test_admin_or_extra_capability_is_refused(self):
        for override in ({"is_admin": True}, {"capabilities": ["launch_triggers", "configure_platform"]}):
            p, calls = self.actor_context(); original = p.http.request
            def request(*args, **kwargs):
                response = original(*args, **kwargs)
                body = json.loads(response[2]); body.update(override)
                return response[0], response[1], json.dumps(body).encode(), response[3]
            p.http.request = request
            with self.assertRaises(Refusal): identity.bind_live(p)
            self.assertFalse(p.identities_bound)
            self.assertTrue(all(method == "GET" for method, _ in calls))

    def test_valid_pair_is_bound_after_both_live_checks(self):
        p, calls = self.actor_context(); identity.bind_live(p)
        self.assertTrue(p.identities_bound); self.assertEqual(len(calls), 4)

    def test_ambient_admin_credentials_do_not_enable_http_writes(self):
        with patch.dict(os.environ, {"NYXID_ACCESS_TOKEN": "ambient", "NYXID_PROFILE": "outside"}):
            http = Http()
            with self.assertRaises(Refusal):
                http.request("https://nyx-api.chrono-ai.fun", "/api/v1/admin/users", "POST")
            self.assertEqual(http.count, 0)
        self.assertFalse(Path(__file__).with_name("nyxid_accounts.py").exists())


if __name__ == "__main__":
    unittest.main()
