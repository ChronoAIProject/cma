"""Offline contracts: unsafe placeholders and uncertain reads cannot reach apply."""

import copy
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("apply_cmaeg", Path(__file__).with_name("apply-cmaeg.py"))
GUARD = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GUARD)


def manifests():
    return [{
        "apiVersion": version, "kind": kind,
        "metadata": {"name": GUARD.NAME, "namespace": GUARD.NAMESPACE},
        **({"data": {key: "" for key in GUARD.OWNER_KEYS}} if kind == "ConfigMap" else {"spec": {}}),
    } for version, kind in (("v1", "ConfigMap"), ("apps/v1", "Deployment"))]


class Cluster:
    def __init__(self, present=(), failure=None):
        self.present = present
        self.failure = failure
        self.calls = []
        self.writes = []

    def command(self, args, body=None):
        self.calls.append(args)
        if args[6] == "get":
            if self.failure:
                return self.failure
            kind = args[7]
            return json.dumps({
                "name": GUARD.NAME, "namespace": GUARD.NAMESPACE,
                "uid": "fixture-" + kind, "resourceVersion": "7", "deleting": False,
                "nonempty": {key: key in self.present for key in GUARD.OWNER_KEYS},
            })
        assert args[6] == "apply"
        self.writes.append((args, json.loads(body)))
        return "resource/fixture"


class ApplyContracts(unittest.TestCase):
    def test_each_live_identifier_blocks_empty_or_whitespace_before_any_write(self):
        for key in GUARD.OWNER_KEYS:
            for empty in ("", " ", "\t"):
                for mode in ("check", "dry-run", "apply"):
                    with self.subTest(key=key, empty=repr(empty), mode=mode):
                        cluster = Cluster(present=[key])
                        candidate = manifests()
                        candidate[0]["data"][key] = empty
                        with self.assertRaisesRegex(GUARD.Refused, key):
                            GUARD.guarded_apply(candidate, "fixture-context", mode, cluster.command)
                        self.assertEqual(cluster.writes, [])

    def test_missing_key_cannot_remove_live_authority(self):
        for key in GUARD.OWNER_KEYS:
            cluster = Cluster(present=[key])
            candidate = manifests()
            del candidate[0]["data"][key]
            with self.assertRaises(GUARD.Refused):
                GUARD.guarded_apply(candidate, "fixture-context", "apply", cluster.command)
            self.assertEqual(cluster.calls, [])

    def test_read_failure_or_uncertainty_never_reaches_apply(self):
        for response in ("", "not-json", "null", "{}", "[]"):
            cluster = Cluster(failure=response or " ")
            with self.assertRaises(GUARD.Refused):
                GUARD.guarded_apply(manifests(), "fixture-context", "apply", cluster.command)
            self.assertEqual(cluster.writes, [])
        def denied(args, body=None):
            raise GUARD.Refused("command refused")
        with self.assertRaises(GUARD.Refused):
            GUARD.guarded_apply(manifests(), "fixture-context", "apply", denied)

    def test_projection_returns_only_presence_and_identity(self):
        template = GUARD.projection("ConfigMap")
        for key in GUARD.OWNER_KEYS:
            self.assertIn('{{if index .data "' + key + '"}}true{{else}}false{{end}}', template)
        self.assertNotIn(".data", GUARD.projection("Deployment"))
        self.assertNotIn("last-applied", template)

    def test_metadata_and_boolean_evidence_are_required(self):
        for change in (
            lambda x: x.update(uid=""), lambda x: x.update(resourceVersion=""),
            lambda x: x.update(name="another"), lambda x: x.update(namespace="another"),
            lambda x: x.update(deleting=True), lambda x: x.update(nonempty={}),
            lambda x: x.update(nonempty={key: "false" for key in GUARD.OWNER_KEYS}),
        ):
            cluster = Cluster()
            def changed(args, body=None):
                result = json.loads(cluster.command(args, body))
                change(result)
                return json.dumps(result)
            with self.assertRaises(GUARD.Refused):
                GUARD.guarded_apply(manifests(), "fixture-context", "apply", changed)
            self.assertEqual(cluster.writes, [])

    def test_empty_live_fields_allow_read_only_check(self):
        cluster = Cluster()
        GUARD.guarded_apply(manifests(), "fixture-context", "check", cluster.command)
        self.assertEqual(len(cluster.calls), 2)
        self.assertEqual(cluster.writes, [])

    def test_apply_uses_captured_bytes_and_uid_resource_version_preconditions(self):
        for mode in ("dry-run", "apply"):
            cluster = Cluster()
            candidate = manifests()
            before = copy.deepcopy(candidate)
            GUARD.guarded_apply(candidate, "fixture-context", mode, cluster.command)
            self.assertEqual(candidate, before)
            self.assertEqual(len(cluster.writes), 2)
            for args, body in cluster.writes:
                self.assertIn("--server-side", args)
                self.assertNotIn("--force-conflicts", args)
                self.assertEqual("--dry-run=server" in args, mode == "dry-run")
                self.assertEqual(body["metadata"]["resourceVersion"], "7")
                self.assertEqual(body["metadata"]["uid"], "fixture-" + body["kind"].lower())

    def test_changed_resource_version_stops_before_deployment_apply(self):
        cluster = Cluster()
        def conflict(args, body=None):
            if body is not None:
                self.assertEqual(json.loads(body)["metadata"]["resourceVersion"], "7")
                raise GUARD.Refused("resource version changed")
            return cluster.command(args, body)
        with self.assertRaises(GUARD.Refused):
            GUARD.guarded_apply(manifests(), "fixture-context", "apply", conflict)
        self.assertEqual(len(cluster.calls), 2)

    def test_wrong_identity_or_context_is_rejected_without_cluster_access(self):
        cluster = Cluster()
        for context in ("", "-other", "two contexts"):
            with self.assertRaises(GUARD.Refused):
                GUARD.guarded_apply(manifests(), context, "apply", cluster.command)
        for field, value in (("kind", "Secret"), ("apiVersion", "v2")):
            candidate = manifests()
            candidate[0][field] = value
            with self.assertRaises(GUARD.Refused):
                GUARD.guarded_apply(candidate, "fixture-context", "apply", cluster.command)
        self.assertEqual(cluster.calls, [])

    def test_subprocess_errors_and_timeout_never_reveal_output(self):
        sentinel = "fixture-private-output"
        result = subprocess.CompletedProcess([], 1, sentinel, sentinel)
        for outcome in (result, subprocess.TimeoutExpired("kubectl", 45, output=sentinel)):
            options = {"side_effect": outcome} if isinstance(outcome, Exception) else {"return_value": outcome}
            with patch.object(GUARD.subprocess, "run", **options):
                with self.assertRaises(GUARD.Refused) as caught:
                    GUARD.invoke(["kubectl"])
                self.assertNotIn(sentinel, str(caught.exception))

    def test_cli_returns_nonzero_with_key_name_only_on_refusal(self):
        stderr, stdout = io.StringIO(), io.StringIO()
        refusal = GUARD.Refused("empty placeholder would erase live " + GUARD.OWNER_KEYS[0])
        with patch.object(GUARD, "load_manifests", return_value=manifests()), \
                patch.object(GUARD, "guarded_apply", side_effect=refusal), \
                redirect_stderr(stderr), redirect_stdout(stdout):
            self.assertEqual(GUARD.main(["--context", "fixture-context", "--apply"]), 1)
        self.assertIn(GUARD.OWNER_KEYS[0], stderr.getvalue())
        self.assertEqual(stdout.getvalue(), "")

    def test_real_cli_loads_shipped_yaml_and_refuses_before_any_apply(self):
        # Exercise the documented command and real YAML loader with an offline kubectl.
        with tempfile.TemporaryDirectory(prefix="cmaeg-apply-contract-") as directory:
            path = Path(directory)
            fake = path / "kubectl"
            fake.write_text(
                "#!" + sys.executable + "\n"
                "import json, os, sys\n"
                "with open(os.environ['FIXTURE_CALLS'], 'a') as f: f.write(sys.argv[6] + '\\n')\n"
                "assert sys.argv[6] == 'get'\n"
                "print(json.dumps({'name':'cma-event-gateway','namespace':'chrono-bot',"
                "'uid':'fixture-object','resourceVersion':'7','deleting':False,"
                "'nonempty':{'CMAEG_OWNER_KEY_SERVICE_ID':True,'CMAEG_OWNER_KEY_CUTOVER_ID':True}}))\n"
            )
            fake.chmod(0o700)
            calls = path / "calls"
            result = subprocess.run([
                sys.executable, str(Path(GUARD.__file__)), "--context", "fixture-context", "--apply",
            ], text=True, capture_output=True, timeout=15, env={
                **os.environ, "PATH": directory + os.pathsep + os.environ["PATH"],
                "FIXTURE_CALLS": str(calls), "PYTHONDONTWRITEBYTECODE": "1",
            })
            self.assertEqual(result.returncode, 1)
            self.assertEqual(result.stdout, "")
            self.assertIn("empty placeholder would erase live " + GUARD.OWNER_KEYS[0], result.stderr)
            self.assertEqual(calls.read_text().splitlines(), ["get", "get"])


if __name__ == "__main__":
    unittest.main()
