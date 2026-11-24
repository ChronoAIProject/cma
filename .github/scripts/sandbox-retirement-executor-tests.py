#!/usr/bin/env python3
"""Exercise the production CLI at separate disposable HTTP boundaries."""

import copy
from datetime import datetime, timedelta, timezone
from hashlib import sha256
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import runpy
import socket
import subprocess
import sys
import tempfile
from threading import Thread
import unittest


SCRIPT = Path(__file__).with_name("sandbox-retirement-executor.py")
EXECUTOR = runpy.run_path(str(SCRIPT))
NAMESPACE = "fixture"
IMAGE = "example.invalid/runtime@sha256:" + "a" * 64
CREATED = "2026-01-01T00:00:00Z"
ACTIVE = "2026-01-01T00:00:01Z"
CUTOFF = "2026-01-02T00:00:00Z"
APP = "/api/v1/admin/sandboxes/sbx_fixture"
CORE = "/api/v1/namespaces/fixture"
SANDBOX = "/apis/agents.x-k8s.io/v1beta1/namespaces/fixture/sandboxes/native"
CLAIM = "/apis/extensions.agents.x-k8s.io/v1beta1/namespaces/fixture/sandboxclaims/claim"
PVC = CORE + "/persistentvolumeclaims/home-native"
PV = "/api/v1/persistentvolumes/volume"
APPROVAL = "synthetic-approval"


def digest(value):
    return sha256(json.dumps(value, sort_keys=True, separators=(",", ":")).encode()).hexdigest()


def json_file(path, body):
    descriptor = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    with os.fdopen(descriptor, "w") as stream:
        json.dump(body, stream)
    return path


def object_body(kind, name, uid, namespaced=True, **fields):
    metadata = {"name": name, "uid": uid}
    if namespaced:
        metadata["namespace"] = NAMESPACE
    return {"kind": kind, "metadata": metadata, **fields}


def absent(path, group):
    kind, name = path.rsplit("/", 2)[-2:]
    return 404, {
        "kind": "Status", "reason": "NotFound", "code": 404,
        "details": {"kind": kind, "name": name, "group": group},
    }


class Listener:
    def __init__(self, callback, token):
        self.calls = []

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_GET(self):
                self.reply(None)

            def do_POST(self):
                data = self.rfile.read(int(self.headers["Content-Length"]))
                self.reply(json.loads(data))

            def reply(self, body):
                method, path = self.command, self.path
                self_record = (method, path, self.headers.get("Authorization"))
                listener.calls.append(self_record)
                if self_record[2] != f"Bearer {token}":
                    status, value = 401, {}
                else:
                    response = callback(method, path, body)
                    if response is None:
                        self.connection.shutdown(socket.SHUT_RDWR)
                        return
                    status, value = response
                data = json.dumps(value, ensure_ascii=False).encode()
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

        listener = self
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.thread = Thread(target=self.server.serve_forever, kwargs={"poll_interval": 0.05}, daemon=True)
        self.thread.start()
        self.origin = f"http://127.0.0.1:{self.server.server_port}"

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(2)


class ExecutorTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="reviewed-retirement-cli-")
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name)
        self.app = Listener(self.app_response, "synthetic-app")
        self.addCleanup(self.app.close)
        self.provider = Listener(self.provider_response, "synthetic-provider")
        self.addCleanup(self.provider.close)
        self.receipt = None
        self.corrupt_receipt = False
        self.replacement = None
        self.guard = None
        self.home = {
            "request": {"sandbox_id": "sbx_fixture"},
            "receipt": {
                "native_authority": {
                    "claim": {"name": "claim", "uid": "claim-uid", "sandbox_name": "native"},
                    "sandbox_uid": "native-uid", "pvc_uid": "pvc-uid",
                },
                "storage_reference": "home-native", "storage_generation": "pvc-uid",
            },
            "released": False,
            "native_pool": {
                "request": {"pool": {"spec": {"image": {"uri": IMAGE}}}},
                "claim": {"uid": "claim-uid"}, "cleaned": False,
            },
        }
        self.runtime = {"runtime_id": "agent-sandbox:native:native-uid", "runtime_version": IMAGE}
        self.snapshot = {
            "id": "sbx_fixture", "owner": "synthetic-用户", "created_at": CREATED, "revision": 7,
            "status": "active", "last_activity_at": ACTIVE, "image": IMAGE,
            "runtime_sha256": digest(self.runtime), "home_sha256": digest(self.home),
        }
        self.sandbox = {
            **self.snapshot, "runtime": self.runtime, "lifecycle": {"home": self.home},
        }
        self.expected = {
            "namespace": NAMESPACE, "storage": "retain_bound",
            "sandbox": {"name": "native", "uid": "native-uid"},
            "claim": {"name": "claim", "uid": "claim-uid"},
            "pvc": {"name": "home-native", "uid": "pvc-uid"},
            "pv": {"name": "volume", "uid": "pv-uid"},
            "pods": [{"name": "worker", "uid": "pod-uid"}],
        }
        self.policy_calls = self.directory / "policy-calls"
        self.policy = self.directory / "policy.py"
        self.policy.write_text(
            f"#!{sys.executable}\n"
            "from pathlib import Path\n"
            "import os, sys\n"
            "assert not any(name.startswith('CMA_RETIREMENT_') for name in os.environ)\n"
            "assert len(sys.argv) == 7 and sys.argv[1:4] == ['policy-check', 'deploy', '--approval']\n"
            "assert sys.argv[4] == 'synthetic-approval' and sys.argv[5] == '--context'\n"
            f"with Path({str(self.policy_calls)!r}).open('a') as log: log.write(sys.argv[6] + '\\n')\n"
            "print('ALLOWED once via reviewed-release-controller')\n",
        )
        self.policy.chmod(0o700)

    def files(self, expires=None, second=False):
        row = {"snapshot": self.snapshot, "provider": self.expected}
        inventory = {"format": 1, "idle_before": CUTOFF, "members": [row]}
        if second:
            inventory["members"].append({**row, "snapshot": {**self.snapshot, "id": "sbx_second"}})
        self.inventory = json_file(self.directory / "inventory.json", inventory)
        self.inventory_sha = sha256(self.inventory.read_bytes()).hexdigest()
        self.member_sha = digest(row)
        if expires is None:
            expires = datetime.now(timezone.utc) + timedelta(minutes=2)
        self.review_body = {
            "format": 1, "verdict": "GO", "executor_sha256": EXECUTOR["source_digest"](),
            "inventory_sha256": self.inventory_sha,
            "origin_sha256": digest({"application": self.app.origin, "provider": self.provider.origin}),
            "dispatch_before": expires.strftime("%Y-%m-%dT%H:%M:%SZ"),
            "approval_sha256": sha256(APPROVAL.encode()).hexdigest(),
        }
        self.review = json_file(self.directory / "review.json", self.review_body)

    def run_cli(self, *authority):
        args = [
            sys.executable, str(SCRIPT), "--application", self.app.origin, "--provider", self.provider.origin,
            "--inventory", str(self.inventory), "--review", str(self.review),
            "--state-dir", str(self.directory), "--observe-seconds", "1", *authority,
        ]
        environment = {
            **os.environ, "PYTHONDONTWRITEBYTECODE": "1", "CMA_RETIREMENT_APPROVAL": APPROVAL,
            "CMA_RETIREMENT_APP_TOKEN": "synthetic-app", "CMA_RETIREMENT_PROVIDER_TOKEN": "synthetic-provider",
        }
        return subprocess.run(args, capture_output=True, text=True, env=environment, timeout=10, check=False)

    def app_response(self, method, path, body):
        if method == "GET" and path == APP:
            return 200, {"sandbox": self.sandbox, "agents": []}
        if method == "POST" and path == APP + "/retire-reviewed":
            self.guard = body
            ordered = {
                "batch_sha256": self.inventory_sha, "member_sha256": self.member_sha,
                "sandbox_id": self.snapshot["id"], "owner": self.snapshot["owner"],
                "created_at": CREATED, "revision": 7, "status": "active", "last_activity_at": ACTIVE,
                "idle_before": CUTOFF, "runtime": self.runtime, "home": self.home,
            }
            guard_bytes = json.dumps(ordered, ensure_ascii=False, separators=(",", ":")).encode()
            self.receipt = {
                "batch_sha256": self.inventory_sha, "member_sha256": self.member_sha,
                "sandbox_id": self.snapshot["id"], "accepted_revision": 8, "event_sequence": 3,
                "guard_sha256": sha256(b"cma-reviewed-retirement-v1\0" + guard_bytes).hexdigest(),
            }
            self.sandbox = {**self.sandbox, "status": "retiring", "revision": 8}
            return None
        if method == "GET" and path == f"{APP}/retire-reviewed/{self.inventory_sha}/{self.member_sha}":
            if self.corrupt_receipt:
                return 200, {**self.receipt, "guard_sha256": "f" * 64}
            return 200, self.receipt
        return 404, {}

    def provider_response(self, method, path, body):
        if method != "GET":
            return 405, {}
        retired = self.sandbox["status"] == "retired"
        if path == SANDBOX:
            if retired:
                return absent(path, "agents.x-k8s.io")
            return 200, object_body("Sandbox", "native", "native-uid")
        if path == CLAIM:
            if retired:
                return absent(path, "extensions.agents.x-k8s.io")
            return 200, object_body("SandboxClaim", "claim", "claim-uid")
        if path == PVC:
            return 200, object_body(
                "PersistentVolumeClaim", "home-native", "pvc-uid",
                spec={"volumeName": "volume"}, status={"phase": "Bound"},
            )
        if path == PV:
            return 200, object_body(
                "PersistentVolume", "volume", "pv-uid", namespaced=False,
                spec={
                    "persistentVolumeReclaimPolicy": "Retain",
                    "claimRef": {"name": "home-native", "uid": "pvc-uid", "namespace": NAMESPACE},
                },
                status={"phase": "Bound"},
            )
        if path == CORE + "/pods":
            pods = []
            if not retired or self.replacement:
                pod = object_body("Pod", "worker", self.replacement or "pod-uid")
                pod["metadata"]["ownerReferences"] = [{
                    "apiVersion": "agents.x-k8s.io/v1beta1", "kind": "Sandbox", "uid": "native-uid",
                }]
                pods.append(pod)
            return 200, {"kind": "PodList", "metadata": {"resourceVersion": "1"}, "items": pods}
        return 404, {}

    def settle(self):
        self.sandbox = copy.deepcopy(self.sandbox)
        self.sandbox.update(status="retired", runtime=None, revision=9)
        self.sandbox["lifecycle"]["home"]["released"] = True

    def test_lost_ack_restart_refuses_wrong_receipt_and_same_name_replacement_then_settles(self):
        self.files()
        first = self.run_cli("--policy-tool", str(self.policy))
        self.assertEqual(first.returncode, 1, first.stderr)
        self.assertEqual(json.loads(first.stdout)["completed_members"], 0)
        self.assertEqual(self.guard["idle_before"], CUTOFF)
        self.assertEqual(self.guard["member_sha256"], self.member_sha)
        self.settle()
        self.corrupt_receipt = True
        bad_receipt = self.run_cli("--observe-only")
        self.assertEqual(bad_receipt.returncode, 1, bad_receipt.stderr)
        self.corrupt_receipt = False
        self.replacement = "replacement-uid"
        replacement = self.run_cli("--observe-only")
        self.assertEqual(replacement.returncode, 1, replacement.stderr)
        self.replacement = None
        settled = self.run_cli("--observe-only")
        self.assertEqual(settled.returncode, 0, settled.stderr)
        self.assertEqual(json.loads(settled.stdout)["completed_members"], 1)
        self.assertEqual(
            [call for call in self.app.calls if call[0] == "POST"],
            [("POST", APP + "/retire-reviewed", "Bearer synthetic-app")],
        )
        self.assertTrue(all(call[0] == "GET" for call in self.provider.calls))
        authority = digest({key: value for key, value in self.review_body.items() if key != "approval_sha256"})
        self.assertEqual(self.policy_calls.read_text().splitlines(), [
            f"old-sandbox-retire/{self.review_body['executor_sha256']}/{self.inventory_sha}/{authority}",
        ])
        journal = (self.directory / f"{self.inventory_sha}.jsonl").read_text()
        self.assertNotIn(self.snapshot["owner"], journal)
        self.assertNotIn(self.snapshot["id"], journal)

    def test_unresolved_submission_stops_later_members(self):
        self.files(second=True)
        first = self.run_cli("--policy-tool", str(self.policy))
        self.assertEqual(first.returncode, 1, first.stderr)
        self.assertTrue(self.app.calls)
        self.assertTrue(all("/sbx_second" not in path for _, path, _ in self.app.calls))

    def test_expired_or_altered_inventory_fails_before_outbound_requests(self):
        self.files(expires=datetime.now(timezone.utc) - timedelta(seconds=1))
        expired = self.run_cli("--policy-tool", str(self.policy))
        self.assertEqual(expired.returncode, 2, expired.stderr)
        with self.inventory.open("a") as stream:
            stream.write(" ")
        altered = self.run_cli("--policy-tool", str(self.policy))
        self.assertEqual(altered.returncode, 2, altered.stderr)
        self.assertEqual((self.app.calls, self.provider.calls), ([], []))
        self.assertFalse(self.policy_calls.exists())


if __name__ == "__main__":
    unittest.main()
