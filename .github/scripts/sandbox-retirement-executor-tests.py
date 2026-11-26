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
RELEASE = {
    "source_sha": "b" * 40,
    "admission_floor_sha": "c" * 40,
    "rollout_evidence_sha256": "d" * 64,
    "version": "1.2.3",
}


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
        self.serving_release = {key: RELEASE[key] for key in ("source_sha", "version")}
        self.controller_approves = True
        self.guard = None
        self.second_sandbox = None
        self.second_receipt = None
        self.approval = APPROVAL
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
        self.runtime = {
            "runtime_id": "agent-sandbox:native:native-uid", "runtime_version": IMAGE, "bound_at": CREATED,
        }
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

    def files(self, expires=None, second=False):
        row = {"snapshot": self.snapshot, "provider": self.expected}
        inventory = {"format": 1, "idle_before": CUTOFF, "members": [row]}
        if second:
            second_home = copy.deepcopy(self.home)
            second_home["request"]["sandbox_id"] = "sbx_second"
            receipt = second_home["receipt"]
            receipt.update(storage_reference="home-native-2", storage_generation="pvc-uid-2")
            origin = receipt["native_authority"]
            origin.update(sandbox_uid="native-uid-2", pvc_uid="pvc-uid-2")
            origin["claim"].update(name="claim-2", uid="claim-uid-2", sandbox_name="native-2")
            second_home["native_pool"]["claim"]["uid"] = "claim-uid-2"
            runtime = {**self.runtime, "runtime_id": "agent-sandbox:native-2:native-uid-2"}
            snapshot = {
                **self.snapshot, "id": "sbx_second", "runtime_sha256": digest(runtime),
                "home_sha256": digest(second_home),
            }
            provider = {
                **self.expected,
                **{kind: {key: value + "-2" for key, value in self.expected[kind].items()}
                   for kind in ("claim", "sandbox", "pvc", "pv")},
                "pods": [{"name": "worker-2", "uid": "pod-uid-2"}],
            }
            second_row = {"snapshot": snapshot, "provider": provider}
            inventory["members"].append(second_row)
            self.second_sandbox = {**snapshot, "runtime": runtime, "lifecycle": {"home": second_home}}
            self.second_member_sha = digest(second_row)
        self.inventory = json_file(self.directory / "inventory.json", inventory)
        self.inventory_sha = sha256(self.inventory.read_bytes()).hexdigest()
        self.member_sha = digest(row)
        if expires is None:
            expires = datetime.now(timezone.utc) + timedelta(minutes=2)
        self.review_body = {
            "format": 1, "verdict": "GO", "executor_sha256": EXECUTOR["source_digest"](),
            "inventory_sha256": self.inventory_sha,
            "origin_sha256": digest({"application": self.app.origin, "provider": self.provider.origin}),
            "state_dir_sha256": digest(str(self.directory.resolve())),
            "dispatch_before": expires.strftime("%Y-%m-%dT%H:%M:%SZ"),
            "approval_sha256": sha256(self.approval.encode()).hexdigest(),
            "serving_release": RELEASE,
            "resume_index": 0,
        }
        self.review = json_file(self.directory / "review.json", self.review_body)

    def run_cli(self, *authority, expire_during_reserve=False):
        harness = """
import json, runpy, sys
from datetime import datetime, timedelta
from pathlib import Path
source, policy_log, approve, cross_deadline = sys.argv[1:5]
sys.argv = [source, *sys.argv[5:]]
sys.path.insert(0, str(Path(source).parent))
def consume(request):
    consumed = Path(policy_log + ".consumed")
    approvals = consumed.read_text().splitlines() if consumed.exists() else []
    approved = approve == "yes" and request["approval"] not in approvals
    with open(policy_log, "a") as log:
        log.write(json.dumps(request) + "\\n")
    if approved:
        with consumed.open("a") as log:
            log.write(request["approval"] + "\\n")
    return approved
main = runpy.run_path(source)["main"]
if cross_deadline == "yes":
    journal = main.__globals__["RetirementJournal"]
    reserve = journal.reserve
    expired = False
    class Clock(datetime):
        @classmethod
        def now(cls, zone):
            return datetime.now(zone) + timedelta(minutes=5 if expired else 0)
    def reserve_and_advance(self, *args):
        global expired
        reserved = reserve(self, *args)
        expired = True
        return reserved
    journal.reserve = reserve_and_advance
    main.__globals__["datetime"] = Clock
raise SystemExit(main(consume))
"""
        args = [
            sys.executable, "-c", harness, str(SCRIPT), str(self.policy_calls),
            "yes" if self.controller_approves else "no",
            "yes" if expire_during_reserve else "no",
            "--application", self.app.origin, "--provider", self.provider.origin,
            "--inventory", str(self.inventory), "--review", str(self.review),
            "--state-dir", str(self.directory), "--observe-seconds", "1", *authority,
        ]
        environment = {
            **os.environ, "PYTHONDONTWRITEBYTECODE": "1", "CMA_RETIREMENT_APPROVAL": self.approval,
            "CMA_RETIREMENT_APP_TOKEN": "synthetic-app", "CMA_RETIREMENT_PROVIDER_TOKEN": "synthetic-provider",
        }
        return subprocess.run(args, capture_output=True, text=True, env=environment, timeout=10, check=False)

    def app_response(self, method, path, body):
        if method == "GET" and path == "/api/v1/build-info":
            return 200, self.serving_release
        if method == "GET" and path == APP:
            return 200, {"sandbox": self.sandbox, "agents": []}
        second = APP.replace("sbx_fixture", "sbx_second")
        if method == "GET" and path == second and self.second_sandbox is not None:
            return 200, {"sandbox": self.second_sandbox, "agents": []}
        if method == "POST" and path == second + "/retire-reviewed":
            encoded = json.dumps(body, ensure_ascii=False, separators=(",", ":")).encode()
            self.second_receipt = {
                "batch_sha256": self.inventory_sha, "member_sha256": self.second_member_sha,
                "sandbox_id": "sbx_second", "accepted_revision": 8, "event_sequence": 3,
                "guard_sha256": sha256(b"cma-reviewed-retirement-v1\0" + encoded).hexdigest(),
            }
            self.second_sandbox.update(status="retired", runtime=None, revision=9)
            self.second_sandbox["lifecycle"]["home"]["released"] = True
            return 200, {"status": "accepted", "receipt": self.second_receipt}
        if method == "GET" and self.second_receipt and path == (
            f"{second}/retire-reviewed/{self.inventory_sha}/{self.second_member_sha}"
        ):
            return 200, self.second_receipt
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
        secondary = path.endswith("-2")
        suffix = "-2" if secondary else ""
        name_path = path.removesuffix(suffix) if secondary else path
        retired = (self.second_sandbox if secondary else self.sandbox)["status"] == "retired"
        if name_path == SANDBOX:
            if retired:
                return absent(path, "agents.x-k8s.io")
            return 200, object_body("Sandbox", "native" + suffix, "native-uid" + suffix)
        if name_path == CLAIM:
            if retired:
                return absent(path, "extensions.agents.x-k8s.io")
            return 200, object_body("SandboxClaim", "claim" + suffix, "claim-uid" + suffix)
        if name_path == PVC:
            return 200, object_body(
                "PersistentVolumeClaim", "home-native" + suffix, "pvc-uid" + suffix,
                spec={"volumeName": "volume" + suffix}, status={"phase": "Bound"},
            )
        if name_path == PV:
            return 200, object_body(
                "PersistentVolume", "volume" + suffix, "pv-uid" + suffix, namespaced=False,
                spec={
                    "persistentVolumeReclaimPolicy": "Retain",
                    "claimRef": {"name": "home-native" + suffix, "uid": "pvc-uid" + suffix,
                                 "namespace": NAMESPACE},
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
            if self.second_sandbox and self.second_sandbox["status"] != "retired":
                pod = object_body("Pod", "worker-2", "pod-uid-2")
                pod["metadata"]["ownerReferences"] = [{
                    "apiVersion": "agents.x-k8s.io/v1beta1", "kind": "Sandbox", "uid": "native-uid-2",
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
        first = self.run_cli("--execute")
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
        requests = [json.loads(line) for line in self.policy_calls.read_text().splitlines()]
        self.assertEqual(requests, [{
            "approval": APPROVAL, "context": f"old-sandbox-retire/{authority}",
            "origin": self.app.origin, "serving_release": RELEASE,
        }])
        self.assertLessEqual(len(requests[0]["context"]), 200)
        journal = (self.directory / f"{self.inventory_sha}.jsonl").read_text()
        self.assertNotIn(self.snapshot["owner"], journal)
        self.assertNotIn(self.snapshot["id"], journal)

    def test_unresolved_submission_stops_later_members(self):
        self.files(second=True)
        first = self.run_cli("--execute")
        self.assertEqual(first.returncode, 1, first.stderr)
        self.assertTrue(self.app.calls)
        self.assertTrue(all("/sbx_second" not in path for _, path, _ in self.app.calls))

    def test_recently_active_member_is_skipped_before_dispatching_next(self):
        self.files(second=True)
        self.sandbox["last_activity_at"] = CUTOFF
        result = self.run_cli("--execute")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)["completed_members"], 2)
        posts = [path for method, path, _ in self.app.calls if method == "POST"]
        self.assertEqual(posts, [APP.replace("sbx_fixture", "sbx_second") + "/retire-reviewed"])
        journal = self.directory / f"{self.inventory_sha}.jsonl"
        records = [json.loads(line) for line in journal.read_text().splitlines()]
        outcomes = [record["outcome"] for record in records if "outcome" in record]
        self.assertEqual(outcomes, ["skipped", "retired"])

    def test_expired_or_altered_inventory_fails_before_outbound_requests(self):
        self.files(expires=datetime.now(timezone.utc) - timedelta(seconds=1))
        expired = self.run_cli("--execute")
        self.assertEqual(expired.returncode, 2, expired.stderr)
        with self.inventory.open("a") as stream:
            stream.write(" ")
        altered = self.run_cli("--execute")
        self.assertEqual(altered.returncode, 2, altered.stderr)
        self.assertEqual((self.app.calls, self.provider.calls), ([], []))
        self.assertFalse(self.policy_calls.exists())

    def test_stale_build_or_controller_refusal_prevents_every_post(self):
        self.files()
        self.serving_release["source_sha"] = "f" * 40
        stale = self.run_cli("--execute")
        self.assertEqual(stale.returncode, 1, stale.stderr)
        self.assertFalse(self.policy_calls.exists())
        self.serving_release["source_sha"] = RELEASE["source_sha"]
        self.controller_approves = False
        denied = self.run_cli("--execute")
        self.assertEqual(denied.returncode, 1, denied.stderr)
        self.assertTrue(self.policy_calls.exists())
        self.assertTrue(all(method == "GET" for method, _, _ in self.app.calls))

    def test_resume_settles_first_member_then_uses_fresh_approval_for_second(self):
        self.files(second=True)
        first = self.run_cli("--execute")
        self.assertEqual(first.returncode, 1, first.stderr)
        self.assertIsNone(self.second_receipt)
        self.settle()
        observe = self.run_cli("--observe-only")
        self.assertEqual(observe.returncode, 1, observe.stderr)
        self.assertEqual(json.loads(observe.stdout)["completed_members"], 1)
        reused = self.run_cli("--execute")
        self.assertEqual(reused.returncode, 2, reused.stderr)
        self.assertIsNone(self.second_receipt)
        self.approval = APPROVAL + "-continuation"
        new_review = {
            **self.review_body, "approval_sha256": sha256(self.approval.encode()).hexdigest(),
            "resume_index": 1,
        }
        self.review = json_file(self.directory / "review-continuation.json", new_review)
        complete = self.run_cli("--execute")
        self.assertEqual(complete.returncode, 0, complete.stderr)
        self.assertEqual(json.loads(complete.stdout)["completed_members"], 2)
        self.assertEqual(
            [path for method, path, _ in self.app.calls if method == "POST"],
            [APP + "/retire-reviewed", APP.replace("sbx_fixture", "sbx_second") + "/retire-reviewed"],
        )
        grants = self.policy_calls.with_suffix(".consumed").read_text().splitlines()
        self.assertEqual(grants, [APPROVAL, self.approval])

    def test_expiry_during_intent_fsync_sends_no_post_and_allows_fresh_grant(self):
        self.files()
        expired = self.run_cli("--execute", expire_during_reserve=True)
        self.assertEqual(expired.returncode, 1, expired.stderr)
        self.assertEqual(json.loads(expired.stdout)["completed_members"], 0)
        self.assertTrue(all(method == "GET" for method, _, _ in self.app.calls))
        journal = self.directory / f"{self.inventory_sha}.jsonl"
        records = [json.loads(line) for line in journal.read_text().splitlines()]
        self.assertIn("guard_sha256", records[-2])
        self.assertEqual(records[-1], {"index": 0, "member_sha256": self.member_sha, "unsent": True})
        reused = self.run_cli("--execute")
        self.assertEqual(reused.returncode, 1, reused.stderr)
        self.assertTrue(all(method == "GET" for method, _, _ in self.app.calls))
        self.approval = APPROVAL + "-fresh"
        new_review = {**self.review_body, "approval_sha256": sha256(self.approval.encode()).hexdigest()}
        self.review = json_file(self.directory / "review-fresh.json", new_review)
        dispatched = self.run_cli("--execute")
        self.assertEqual(dispatched.returncode, 1, dispatched.stderr)
        self.settle()
        observed = self.run_cli("--observe-only")
        self.assertEqual(observed.returncode, 0, observed.stderr)
        self.assertEqual(json.loads(observed.stdout)["completed_members"], 1)
        self.assertEqual(
            [path for method, path, _ in self.app.calls if method == "POST"],
            [APP + "/retire-reviewed"],
        )


if __name__ == "__main__":
    unittest.main()
