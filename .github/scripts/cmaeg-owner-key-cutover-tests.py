#!/usr/bin/env python3
"""Offline cutover contracts: fake Kubernetes only, no cluster or secret access."""
import base64
import contextlib
import copy
import datetime as dt
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import unittest
from unittest import mock
import uuid

import cmaeg_owner_key_cutover as model

SPEC = importlib.util.spec_from_file_location("cutover", Path(__file__).with_name("cmaeg-owner-key-cutover.py"))
cli = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(cli)
ROOT = Path(__file__).resolve().parents[2]


def meta(name):
    return {"name": name, "namespace": model.NAMESPACE, "uid": name + "-fixture", "resourceVersion": "1"}


def fixture():
    cm = {"apiVersion": "v1", "kind": "ConfigMap", "metadata": meta(model.NAME), "data": {
        "CMAEG_OWNER_KEYS_ENABLED": "true", "CMAEG_WORKER_CREDENTIAL_TRACKING_ENABLED": "true",
        "unrelated": "preserve", **{k: str(uuid.UUID(int=i + 1)) for i, k in enumerate(model.KEYS)}}}
    dep = {"apiVersion": "apps/v1", "kind": "Deployment", "metadata": {**meta(model.NAME), "generation": 71},
           "spec": {"replicas": 1, "template": {"spec": {"containers": [{"name": "cmaeg",
                     "image": "registry.invalid/cmaeg@sha256:" + "a" * 64,
                     "envFrom": [{"configMapRef": {"name": model.NAME}}],
                     "env": [{"name": "UNCHANGED", "value": "fixture"}]}]}}}}
    return cm, dep, None


def apply_patch(obj, patch):
    for op in patch:
        path = [s.replace("~1", "/").replace("~0", "~") for s in op["path"].split("/")[1:]]
        parent = obj
        for key in path[:-1]:
            parent = parent[int(key)] if isinstance(parent, list) else parent[key]
        key = int(path[-1]) if isinstance(parent, list) else path[-1]
        if op["op"] == "test":
            model.require(parent[key] == op["value"], "fixture_conflict")
        elif op["op"] == "remove":
            del parent[key]
        else:
            parent[key] = copy.deepcopy(op["value"])
    obj["metadata"]["resourceVersion"] = str(int(obj["metadata"]["resourceVersion"]) + 1)
    if obj["kind"] == "Deployment":
        obj["metadata"]["generation"] += 1


class Fake:
    context = "fixture-context"

    def __init__(self):
        self.state = list(fixture())
        self.writes = []
        self.reads = 0
        self.race = None
        self.uncertain = False

    def read(self):
        self.reads += 1
        if self.race:
            self.race(self)
        return copy.deepcopy(self.state)

    def create_secret(self, body):
        model.require(self.state[2] is None, "fixture_conflict")
        self.state[2] = copy.deepcopy(body)
        self.state[2]["metadata"].update(meta(model.SECRET))
        self.writes.append(("secret", copy.deepcopy(body)))
        if self.uncertain:
            raise model.Refused("kubernetes_request_failed_or_uncertain")

    def patch(self, kind, name, patch):
        model.require(name == model.NAME, "fixture_wrong_target")
        index = {"configmap": 0, "deployment": 1}[kind]
        apply_patch(self.state[index], patch)
        self.writes.append((kind, copy.deepcopy(patch)))

    def run(self, phase, receipt=None):
        return cli.execute(self, phase, model.projection(self.state, self.context), receipt)

    def healthy_receipt(self):
        dep, secret = self.state[1:]
        dep["status"] = {"observedGeneration": dep["metadata"]["generation"], "replicas": 1,
                         "updatedReplicas": 1, "readyReplicas": 1, "availableReplicas": 1}
        return {"deployment_uid": dep["metadata"]["uid"], "generation": dep["metadata"]["generation"],
                "secret": model.identity(secret, "Secret", model.SECRET),
                "owner_key_read_passed": True, "existing_channels_passed": True, "observation_passed": True,
                "verified_at": dt.datetime.now(dt.timezone.utc).isoformat()}


class Contracts(unittest.TestCase):
    def test_value_preserving_cutover_is_fenced_and_idempotent(self):
        client = Fake()
        original = copy.deepcopy(client.state[0]["data"])
        original_spec = copy.deepcopy(client.state[1]["spec"])
        client.run("seed")
        client.run("seed")
        self.assertEqual(len(client.writes), 1)
        self.assertEqual(model.secret_pair(client.state[2]), {k: original[k] for k in model.KEYS})
        client.run("switch")
        client.run("switch")
        self.assertEqual(len(client.writes), 2)
        expected = copy.deepcopy(original_spec)
        expected["template"]["spec"]["containers"][0]["env"] += model.references()
        self.assertEqual(client.state[1]["spec"], expected)
        receipt = client.healthy_receipt()
        client.run("cleanup", receipt)
        client.run("cleanup", receipt)
        self.assertEqual(len(client.writes), 3)
        self.assertTrue(client.run("verify")["references_present"])
        self.assertEqual(client.state[0]["data"], {k: v for k, v in original.items() if k not in model.KEYS})
        for kind, patch in client.writes[1:]:
            self.assertEqual([p["path"] for p in patch[:2]], ["/metadata/uid", "/metadata/resourceVersion"])
            self.assertFalse(any(p["op"] != "test" and p["path"].endswith("/image") for p in patch))

    def test_missing_empty_malformed_and_half_removed_identifiers_refuse_before_writes(self):
        for key in model.KEYS:
            for invalid in [None, "", " ", "not-a-value", str(uuid.UUID(int=0))]:
                client = Fake()
                if invalid is None:
                    del client.state[0]["data"][key]
                else:
                    client.state[0]["data"][key] = invalid
                with self.assertRaises(model.Refused):
                    client.run("seed")
                self.assertEqual(client.writes, [])

    def test_existing_secret_collision_or_invalid_payload_never_overwrites(self):
        for mutation in [lambda s: s["data"].update({model.KEYS[0]: base64.b64encode(str(uuid.UUID(int=9)).encode()).decode()}),
                         lambda s: s["data"].update({"EXTRA": ""}),
                         lambda s: s["data"].update({model.KEYS[0]: "invalid-base64"}),
                         lambda s: s.update({"type": "wrong-type"})]:
            client = Fake()
            client.run("seed")
            mutation(client.state[2])
            before = copy.deepcopy(client.state[2])
            with self.assertRaises(model.Refused):
                client.run("seed")
            self.assertEqual(client.state[2], before)
            self.assertEqual(len(client.writes), 1)

    def test_lost_create_response_requires_recapture_and_does_not_repeat_creation(self):
        client = Fake()
        client.uncertain = True
        with self.assertRaises(model.Refused):
            client.run("seed")
        client.run("seed")
        self.assertEqual(len(client.writes), 1)

    def test_stale_uid_version_generation_or_context_refuses(self):
        for field, key in [("configmap", "uid"), ("configmap", "resourceVersion"),
                           ("deployment", "generation"), ("deployment", "resourceVersion")]:
            client = Fake()
            fence = model.projection(client.state, client.context)
            fence[field][key] = "stale"
            with self.assertRaises(model.Refused):
                cli.execute(client, "seed", fence)
            self.assertEqual(client.writes, [])
        client = Fake()
        fence = model.projection(client.state, "wrong-context")
        with self.assertRaises(model.Refused):
            cli.execute(client, "seed", fence)

    def test_terminating_dependencies_refuse_without_mutation(self):
        for index in range(3):
            client = Fake()
            client.run("seed")
            client.state[index]["metadata"]["deletionTimestamp"] = "fixture"
            with self.assertRaisesRegex(model.Refused, "object_terminating"):
                client.run("switch")
            self.assertEqual(len(client.writes), 1)

    def test_concurrent_dependency_write_refuses_before_deployment_patch(self):
        for index in [0, 2]:
            client = Fake()
            client.run("seed")
            client.reads = 0
            client.race = lambda c: c.state[index]["metadata"].update({"resourceVersion": "2"}) if c.reads == 2 else None
            with self.assertRaisesRegex(model.Refused, "dependency_changed_hold"):
                client.run("switch")
            self.assertEqual(len(client.writes), 1)

    def test_existing_literal_duplicate_or_wrong_reference_is_refused(self):
        for entries in [[{"name": model.KEYS[0], "value": "fixture"}], model.references() * 2,
                        [{"name": k, "valueFrom": {"configMapKeyRef": {"name": model.NAME, "key": k}}} for k in model.KEYS]]:
            client = Fake()
            client.run("seed")
            client.state[1]["spec"]["template"]["spec"]["containers"][0]["env"] += entries
            with self.assertRaises(model.Refused):
                client.run("switch")
            self.assertEqual(len(client.writes), 1)

    def test_post_write_dependency_race_holds_without_retrying(self):
        for phase, changed in [("seed", 0), ("switch", 2), ("cleanup", 1)]:
            client = Fake()
            receipt = None
            if phase != "seed":
                client.run("seed")
            if phase == "cleanup":
                client.run("switch")
                receipt = client.healthy_receipt()
            writes = len(client.writes)
            client.reads = 0
            def race(c):
                if c.reads == 3:
                    c.state[changed]["metadata"]["resourceVersion"] = "changed-after-write"
            client.race = race
            with self.assertRaisesRegex(model.Refused, "dependency_changed_hold"):
                client.run(phase, receipt)
            self.assertEqual(len(client.writes), writes + 1)

    def test_failed_read_or_patch_never_retries_or_cleans_source(self):
        client = Fake()
        client.run("seed")
        original = copy.deepcopy(client.state)
        failure = model.Refused("kubernetes_request_failed_or_uncertain")
        with mock.patch.object(client, "read", side_effect=failure) as read:
            with self.assertRaises(model.Refused):
                client.run("switch")
            self.assertEqual(read.call_count, 1)
        with mock.patch.object(client, "patch", side_effect=failure) as patch:
            with self.assertRaises(model.Refused):
                client.run("switch")
            self.assertEqual(patch.call_count, 1)
        self.assertEqual(client.state, original)

    def test_secret_change_invalidates_owner_verification_receipt(self):
        client = Fake()
        client.run("seed")
        client.run("switch")
        receipt = client.healthy_receipt()
        client.state[2]["metadata"]["resourceVersion"] = "new-version"
        with self.assertRaisesRegex(model.Refused, "verification_fence_mismatch"):
            client.run("cleanup", receipt)
        self.assertEqual(len(client.writes), 2)
        self.assertTrue(all(k in client.state[0]["data"] for k in model.KEYS))

    def test_cleanup_requires_ready_and_matching_recent_owner_verification(self):
        client = Fake()
        client.run("seed")
        client.run("switch")
        with self.assertRaisesRegex(model.Refused, "rollout_not_verified"):
            client.run("cleanup", {})
        receipt = client.healthy_receipt()
        for key, bad in [("owner_key_read_passed", False), ("generation", 1),
                         ("verified_at", "2000-01-01T00:00:00Z"), ("secret", {})]:
            with self.assertRaises(model.Refused):
                client.run("cleanup", {**receipt, key: bad})
        self.assertTrue(all(k in client.state[0]["data"] for k in model.KEYS))
        self.assertEqual(len(client.writes), 2)

    def test_legacy_last_applied_copy_is_removed_without_copying_it_to_evidence(self):
        client = Fake()
        client.state[0]["metadata"]["annotations"] = {model.LAST_APPLIED: json.dumps(client.state[0]), "unrelated": "keep"}
        client.run("seed")
        client.run("switch")
        client.run("cleanup", client.healthy_receipt())
        self.assertEqual(client.state[0]["metadata"]["annotations"], {"unrelated": "keep"})
        public = json.dumps(client.run("verify"))
        for value in model.secret_pair(client.state[2]).values():
            self.assertNotIn(value, public)
            self.assertNotIn(base64.b64encode(value.encode()).decode(), public)

    def test_kubectl_payloads_use_stdin_and_errors_are_fixed(self):
        body = model.seed_plan(fixture()[0], None)
        with mock.patch.object(cli.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, b"secret/fixture", b"")) as run:
            cli.Kubectl("fixture").create_secret(body)
            args, kwargs = run.call_args
            self.assertNotIn("shell", kwargs)
            self.assertEqual(kwargs["stderr"], subprocess.DEVNULL)
            self.assertEqual(json.loads(kwargs["input"]), body)
            for value in model.secret_pair({**body, "metadata": meta(model.SECRET)}).values():
                self.assertNotIn(value, " ".join(args[0]))
        with mock.patch.object(cli.subprocess, "run", return_value=subprocess.CompletedProcess([], 1, b"private-output", b"private-error")):
            with self.assertRaisesRegex(model.Refused, "^kubernetes_request_failed_or_uncertain$"):
                cli.Kubectl("fixture").create_secret(body)

    def test_main_never_prints_json_parser_excerpts(self):
        output, error = io.StringIO(), io.StringIO()
        with mock.patch.object(cli.sys, "argv", ["cutover", "capture", "--context", "fixture"]), \
             mock.patch.object(cli.Kubectl, "read", side_effect=json.JSONDecodeError("private-canary", "private-canary", 0)), \
             contextlib.redirect_stdout(output), contextlib.redirect_stderr(error):
            self.assertEqual(cli.main(), 1)
        self.assertEqual(output.getvalue(), "")
        self.assertNotIn("private-canary", error.getvalue())

    def test_repository_manifests_have_only_the_secret_reference_contract(self):
        ruby = 'require "yaml"; require "json"; puts JSON.generate(ARGV.flat_map { |p| YAML.load_stream(File.read(p)) })'
        paths = [ROOT / "infra/cmaeg" / name for name in ("configmap.yaml", "deployment.yaml", "secret.yaml")]
        documents = json.loads(subprocess.check_output(["ruby", "-e", ruby, *map(str, paths)]))
        cm, dep, *secrets = documents
        self.assertTrue(all(key not in cm["data"] for key in model.KEYS))
        env = next(c for c in dep["spec"]["template"]["spec"]["containers"] if c["name"] == "cmaeg")["env"]
        self.assertEqual([e for e in env if e["name"] in model.KEYS], model.references())
        dedicated = [s for s in secrets if s and s["metadata"]["name"] == model.SECRET]
        self.assertEqual(len(dedicated), 1)
        self.assertEqual(dedicated[0]["stringData"], {})
        self.assertNotIn("data", dedicated[0])


if __name__ == "__main__":
    unittest.main()
