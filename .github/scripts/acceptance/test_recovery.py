"""Crash injection across Launch submission, resource cleanup and logout."""

import contextlib
import io
import json
import unittest
from unittest.mock import patch

import launch_ledger
import launch_recovery
import session_journal
from identity import fingerprint
from safety import Refusal
from test_support import context, intent, launch_value, ordinary, restart


def response(status, body=None):
    return status, {}, json.dumps(body or {}).encode(), 0


class LaunchServer:
    def __init__(self, fault=None):
        self.keys = {}; self.calls = []; self.launch = None; self.sandbox = False
        self.fault = fault; self.tripped = False

    def trip(self, point):
        if self.fault == point and not self.tripped:
            self.tripped = True
            raise Refusal("synthetic_lost_response")

    def request(self, origin, path, method="GET", token=None, body=None, headers=None, **kwargs):
        self.calls.append((method, path))
        if method == "POST" and path.endswith("/launches"):
            key = headers["Idempotency-Key"]
            if key not in self.keys:
                self.keys[key] = launch_value(True)
                self.launch = launch_value(True); self.sandbox = True
            self.trip("create")
            return response(201, self.keys[key])
        if path == "/api/v1/launches/test-launch" and method == "GET":
            return response(200, self.launch)
        if path.endswith("/cancel") and method == "POST":
            self.launch["state"] = "cancelled"; self.trip("cancel")
            return response(200, self.launch)
        if path == "/api/v1/sandboxes/test-sandbox":
            if method == "DELETE":
                self.sandbox = False; self.trip("delete")
                return response(202)
            return response(200, {"id": "test-sandbox", "owner": "test-a"}) if self.sandbox else response(404)
        if path.startswith("/api/v1/sandboxes?"):
            return response(200, {"items": [{"id": "test-sandbox"}] if self.sandbox else []})
        if path.startswith("/api/v1/launches?"):
            return response(200, {"items": [{"id": "test-launch"}] if self.launch else []})
        raise AssertionError("unexpected_mock_route")


class RecoveryTests(unittest.TestCase):
    def exercise_launch(self, fault=None, persistence=None, after=False):
        with context() as p, contextlib.redirect_stdout(io.StringIO()), patch("launch_recovery.time.sleep"):
            server = LaunchServer(fault); p.http.request = server.request
            writes = 0; tripped = False
            real_save = p.save
            real_ledger_write = launch_ledger.write_private
            def checkpoint(call):
                nonlocal writes, tripped
                writes += 1
                should_trip = writes == persistence and not tripped
                if should_trip and not after:
                    tripped = True; raise Refusal("synthetic_persistence_failure")
                call()
                if should_trip:
                    tripped = True; raise Refusal("synthetic_persistence_failure")
            p.save = lambda: checkpoint(real_save)
            with patch("launch_ledger.write_private", side_effect=lambda n, v: checkpoint(lambda: real_ledger_write(n, v))):
                try:
                    draft = intent()
                    result = p.write("create", "a", draft["path"], "POST", draft["body"])
                    launch_recovery.remember(p, "a", json.loads(result[2]))
                    launch_recovery.cleanup(p, "a")
                except Refusal:
                    pass
            p.save = real_save; restart(p)
            # A new invocation resumes original obligations before inventory gates.
            for _ in range(3):
                try:
                    launch_recovery.cleanup(p, "a")
                    break
                except Refusal:
                    restart(p)
            self.assertLessEqual(len(server.keys), 1)
            self.assertFalse(server.sandbox)
            self.assertFalse(launch_ledger.pending(p, "a"))
            if server.launch:
                self.assertEqual(server.launch["state"], "cancelled")
            if fault == "create":
                methods = [m for m, path in server.calls if path.endswith("/launches")]
                self.assertEqual(methods, ["POST", "POST"])
                dispatched = [i for i, (_, path) in enumerate(server.calls) if path.endswith("/launches")]
                self.assertFalse(any(path.startswith("/api/v1/sandboxes?")
                                     for _, path in server.calls[dispatched[0]:dispatched[1]]))
            return writes

    def test_lost_create_cancel_delete_responses_reconcile_exact_resource(self):
        for fault in ("create", "cancel", "delete"):
            with self.subTest(fault=fault): self.exercise_launch(fault=fault)

    def test_every_launch_persistence_boundary_is_resumable(self):
        count = self.exercise_launch()
        self.assertGreater(count, 8)
        for point in range(1, count + 1):
            for after in (False, True):
                with self.subTest(point=point, after=after):
                    self.exercise_launch(persistence=point, after=after)

    def exercise_logout(self, persistence=None, after=False, response_loss=False):
        with context() as p, contextlib.redirect_stdout(io.StringIO()):
            session_journal.register_current(p)
            key = fingerprint(p.accounts["a"]["access_token"])
            live = True; logouts = 0; calls = []; loss = False
            def request(origin, path, method="GET", token=None, **kwargs):
                nonlocal live, logouts, loss
                calls.append((method, path))
                if path == "/api/v1/users/me":
                    return response(200, ordinary("test-a")) if live else response(401)
                if path == "/auth/session":
                    return response(200, {"subject": "test-a", "authenticated": True, "capabilities": ["launch_triggers"]})
                if path == "/api/v1/auth/logout" and method == "POST":
                    self.assertEqual(token, p.accounts["a"]["access_token"])
                    logouts += 1; live = False
                    if response_loss and not loss:
                        loss = True; raise Refusal("synthetic_lost_logout_reply")
                    return response(200)
                raise AssertionError("unexpected_session_route")
            p.http.request = request
            real_write = session_journal.write_private; count = 0
            def save(name, value):
                nonlocal count
                count += 1
                if persistence == count and not after: raise Refusal("synthetic_before_save")
                real_write(name, value)
                if persistence == count: raise Refusal("synthetic_after_save")
            with patch("session_journal.write_private", side_effect=save):
                try: session_journal.revoke_one(p, key)
                except Refusal: pass
            session_journal.revoke_one(p, key)
            self.assertFalse(live); self.assertEqual(logouts, 1)
            self.assertEqual(session_journal.load(p)["tokens"][key]["status"], "revoked")
            self.assertTrue(all("/sessions" not in path for _, path in calls))
            return count

    def test_finish_submitted_means_uncertain_not_completed(self):
        for point in (1, 2):
            for after in (False, True):
                with self.subTest(point=point, after=after): self.exercise_logout(point, after)
        self.exercise_logout(response_loss=True)

    def test_foreign_or_privileged_journaled_session_never_revokes(self):
        with context() as p:
            session_journal.register_current(p)
            key = fingerprint(p.accounts["a"]["access_token"])
            for override in ({"id": "unrelated"}, {"is_admin": True}, {"role": None}):
                calls = []
                def request(origin, path, method="GET", **kwargs):
                    calls.append(method)
                    return response(200, dict(ordinary("test-a"), **override))
                p.http.request = request
                with self.assertRaises(Refusal): session_journal.revoke_one(p, key)
                self.assertEqual(calls, ["GET"])


if __name__ == "__main__":
    unittest.main()
