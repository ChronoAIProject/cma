#!/usr/bin/env python3
"""Exercise the preflight CLI through a disposable admin listener."""

import copy
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


CLI = Path(__file__).with_name("sandbox-retirement-preflight.py")
OLD = "2026-01-01T00:00:00Z"
CUTOFF = "2026-01-02T00:00:00Z"
RECENT = "2026-01-03T00:00:00Z"
IMAGE = "example.test/runtime@sha256:" + "a" * 64


def digest(value):
    encoded = json.dumps(value, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(encoded).hexdigest()


class PreflightTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="sandbox-retirement-preflight-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.inventory = self.root / "inventory.json"
        self.output = self.root / "result.json"
        self.members = []
        replies = {}
        self.requests = []
        requests = self.requests

        class Admin(BaseHTTPRequestHandler):
            def log_message(self, *_):
                return

            def do_GET(self):
                requests.append((self.command, self.path, self.headers.get("Authorization")))
                code, body = replies[self.path]
                self.send_response(code)
                self.end_headers()
                self.wfile.write(json.dumps(body).encode())

            def do_POST(self):
                requests.append((self.command, self.path, self.headers.get("Authorization")))
                self.send_error(405)

        self.replies = replies
        server = ThreadingHTTPServer(("127.0.0.1", 0), Admin)
        worker = threading.Thread(target=server.serve_forever)
        worker.start()
        self.addCleanup(worker.join, 5)
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        self.origin = f"http://127.0.0.1:{server.server_port}"

    def add_member(self, name):
        sandbox = {
            "id": f"sbx_{name}",
            "owner": "synthetic-owner",
            "status": "active",
            "created_at": OLD,
            "last_activity_at": OLD,
            "revision": 4,
            "runtime": {"runtime_id": f"agent-sandbox:{name}", "runtime_version": IMAGE},
            "lifecycle": {
                "home": {
                    "released": False,
                    "receipt": {"storage_reference": f"home-{name}"},
                    "native_pool": {
                        "request": {"pool": {"spec": {"image": {"uri": IMAGE}}}},
                        "claim": {"uid": f"synthetic-claim-{name}"},
                        "cleaned": False,
                    },
                },
            },
        }
        member = {key: sandbox[key] for key in ("id", "owner", "status", "created_at", "last_activity_at", "revision")}
        member.update(
            image=IMAGE,
            runtime_sha256=digest(sandbox["runtime"]),
            home_sha256=digest(sandbox["lifecycle"]["home"]),
        )
        self.members.append(member)
        path = f"/api/v1/admin/sandboxes/{member['id']}"
        self.replies[path] = (200, {"sandbox": sandbox, "agents": [{"status": "ready", "updated_at": OLD}]})
        return path, sandbox

    def run_cli(self):
        self.inventory.write_text(json.dumps({"format": 1, "idle_before": CUTOFF, "members": self.members}))
        self.inventory.chmod(0o600)
        return subprocess.run(
            [sys.executable, str(CLI), self.origin, str(self.inventory), str(self.output)],
            env={**os.environ, "CMA_RETIREMENT_READ_TOKEN": "synthetic-read-credential"},
            capture_output=True, text=True, timeout=12,
        )

    def test_reads_in_order_and_reports_hashes_without_submitting(self):
        self.add_member("eligible")
        _, stale = self.add_member("stale")
        stale["revision"] += 1
        _, recent = self.add_member("recent")
        recent["last_activity_at"] = RECENT
        _, foreign = self.add_member("foreign")
        foreign["owner"] = "different-owner"
        _, retiring = self.add_member("retiring")
        retiring.update(status="retiring", revision=5)
        _, replaced = self.add_member("replaced")
        replaced["lifecycle"]["home"]["native_pool"]["claim"]["uid"] = "replacement-claim"
        busy, _ = self.add_member("busy")
        self.replies[busy][1]["agents"][0]["status"] = "working"
        _, unbound = self.add_member("unbound")
        del unbound["lifecycle"]["home"]["receipt"]
        _, other_image = self.add_member("other-image")
        other_image["runtime"]["runtime_version"] = "example.test/runtime@sha256:" + "b" * 64
        missing, _ = self.add_member("hidden")
        self.replies[missing] = (404, {})

        result = self.run_cli()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, "")
        report = json.loads(self.output.read_text())
        self.assertEqual(report["inventory_sha256"], hashlib.sha256(self.inventory.read_bytes()).hexdigest())
        self.assertFalse(report["execution_authorized"])
        self.assertEqual(report["rows"], [
            {"member_sha256": digest(member), "outcome": outcome}
            for member, outcome in zip(self.members, [
                "eligible_for_review", "snapshot_changed", "recent_or_active_work", "identity_changed",
                "observe_existing_retirement", "runtime_or_home_changed", "recent_or_active_work",
                "runtime_or_home_unavailable", "runtime_or_home_changed", "not_found_or_hidden",
            ])
        ])
        self.assertEqual(self.requests, [
            ("GET", f"/api/v1/admin/sandboxes/{member['id']}", "Bearer synthetic-read-credential")
            for member in self.members
        ])
        self.assertEqual(self.output.stat().st_mode & 0o777, 0o600)

    def test_refuses_duplicate_inventory_before_network(self):
        self.add_member("double")
        self.members.append(copy.deepcopy(self.members[0]))
        self.assertEqual(self.run_cli().returncode, 2)
        self.assertEqual(self.requests, [])
        self.assertFalse(self.output.exists())

    def test_unavailable_read_is_reported_with_failure_exit(self):
        broken, _ = self.add_member("broken")
        self.replies[broken] = (503, {"private": "upstream detail"})
        result = self.run_cli()
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertEqual(json.loads(self.output.read_text())["rows"], [
            {"member_sha256": digest(self.members[0]), "outcome": "observation_unavailable"},
        ])


if __name__ == "__main__":
    unittest.main()
