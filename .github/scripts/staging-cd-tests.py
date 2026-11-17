#!/usr/bin/env python3
"""Offline success/failure regressions for staging image promotion."""
import copy
import hashlib
import importlib.util
import io
import json
import os
import subprocess
import tempfile
import threading
import unittest
import urllib.error
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
from unittest.mock import Mock, patch

SPEC = importlib.util.spec_from_file_location("staging_cd", Path(__file__).with_name("staging-cd.py"))
cd = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(cd)
SHA = "a" * 40
IMAGE = "sha256:" + "b" * 64
MANIFEST = "sha256:" + "c" * 64
RUN = {"head_sha": SHA, "head_branch": "develop", "event": "push", "conclusion": "success",
       "status": "completed", "path": ".github/workflows/ci.yml", "id": 123, "run_attempt": 2,
       "repository": {"full_name": cd.REPO}, "head_repository": {"full_name": cd.REPO}}


def reference(target):
    return f"ghcr.io/chronoaiproject/cma-staging-{target[3]}@{MANIFEST}"


def app_view(target):
    return {"name": target[1], "project": "cma", "owner": cd.SCOPE, "namespace": "chrono-ai-staging",
            "generation": 7, "url": "https://example.invalid", "status": {"phase": "applied"},
            "spec": {"addons": {"mongodb": {}}, "services": {target[2]: {
                "image": "ghcr.io/chronoaiproject/previous:old", "image_digest": IMAGE,
                "port": 8080, "replicas": 2, "routes": ["/"], "secret_keys": ["Z_KEY", "A_KEY"],
                "env": {"MODE": "staging"}, "resources": {"memory_limit": "1Gi"},
                "volumes": [{"name": "data", "type": "persistent", "size": "1Gi"}],
                "health_path": "/readyz", "ingress_from_namespaces": ["monitoring"]}}},
            "secret_keys_present": {target[2]: ["Z_KEY", "A_KEY"]}}


def rows():
    return {t[1]: {"image": "not published", "phase": "not checked"} for t in cd.TARGETS}


class FakeCD:
    def __init__(self):
        self.views = {t[1]: app_view(t) for t in cd.TARGETS}
        self.statuses = {t[1]: {"phase": "ready"} for t in cd.TARGETS}
        self.calls = []

    def request(self, app, method="GET", **kwargs):
        self.calls.append((app, method, copy.deepcopy(kwargs)))
        if method == "PUT":
            assert kwargs["generation"] == self.views[app]["generation"]
            self.views[app]["generation"] += 1
            self.views[app]["spec"] = copy.deepcopy(kwargs["body"])
        return copy.deepcopy(self.statuses[app] if kwargs.get("status") else self.views[app])


class PromotionTests(unittest.TestCase):
    def test_full_spec_put_changes_only_image_fields(self):
        for target in cd.TARGETS:
            original = app_view(target)
            original["spec"]["services"]["sidecar"] = {"image": "ghcr.io/example/sidecar:1", "image_digest": IMAGE, "replicas": 0}
            retained = copy.deepcopy(original)
            body = cd.replacement_spec(original, target, reference(target))
            expected = copy.deepcopy(original["spec"])
            for service in expected["services"].values():
                del service["image_digest"]
            expected["services"][target[2]]["image"] = reference(target)
            self.assertEqual(body, expected)
            self.assertEqual(original, retained, "GET snapshot must remain immutable")
            self.assertNotIn("generation", body)
            with self.assertRaises(cd.Refusal):
                cd.replacement_spec(original, target, "mutable:tag")

    def test_put_sequence_preserves_spec_and_generation(self):
        client, report = FakeCD(), rows()
        original = copy.deepcopy(client.views)
        def publish(images, sha, output):
            self.assertEqual(len(client.calls), 3, "all apps preflight before publication")
            for target in cd.TARGETS:
                output[target[1]]["image"] = reference(target)
        cd.promote(client, {}, SHA, report, False, publisher=publish)
        puts = [call for call in client.calls if call[1] == "PUT"]
        self.assertEqual(len(puts), 3)
        for target, (app, _, kwargs) in zip(cd.TARGETS, puts):
            self.assertEqual(kwargs["generation"], 7)
            self.assertEqual(kwargs["body"], cd.replacement_spec(original[app], target, reference(target)))
            self.assertEqual(report[app]["phase"], "ready")

    def test_dry_run_reads_every_app_without_writes_or_publication(self):
        client, publisher, report = FakeCD(), Mock(), rows()
        cd.promote(client, {}, SHA, report, True, publisher=publisher)
        publisher.assert_not_called()
        self.assertEqual({call[1] for call in client.calls}, {"GET"})
        self.assertEqual(len([c for c in client.calls if c[2].get("status")]), 3)
        self.assertTrue(all(r["image"] == "not published" for r in report.values()))

    def test_preflight_failure_prevents_publication(self):
        client, publisher = FakeCD(), Mock()
        client.views["cma-am-staging"]["owner"] = "other"
        with self.assertRaises(cd.Refusal):
            cd.promote(client, {}, SHA, rows(), False, publisher=publisher)
        publisher.assert_not_called()

    def test_fail_closed_app_identity_and_concurrent_change(self):
        target = cd.TARGETS[0]
        for field, value in (("generation", True), ("project", "other"), ("namespace", "production"), ("name", "wrong")):
            view = app_view(target)
            view[field] = value
            with self.subTest(field=field), self.assertRaises(cd.Refusal):
                cd.validate_app(view, target)
        with self.assertRaises(cd.Refusal):
            cd.validate_app(app_view(target), target, reference(target), 8)

    def test_status_errors_timeout_and_unknown_fail(self):
        for phase in ("error", "degraded", "deleting", "invented"):
            client = FakeCD()
            client.statuses["cma-staging"] = {"phase": phase}
            with self.subTest(phase=phase), self.assertRaises(cd.Refusal):
                cd.rollout(client, cd.TARGETS, rows(), True)
        client = FakeCD()
        client.statuses["cma-staging"] = {"phase": "progressing"}
        now = [0]
        def sleep(seconds):
            now[0] += seconds
        with self.assertRaisesRegex(cd.Refusal, "ten minutes"):
            cd.rollout(client, cd.TARGETS, rows(), True, clock=lambda: now[0], sleep=sleep)
        self.assertEqual(now[0], 600)

    def test_missing_keys_are_narrowly_nonfatal(self):
        client, report = FakeCD(), rows()
        for target in cd.TARGETS[1:]:
            view = client.views[target[1]]
            view["secret_keys_present"] = {}
            last = {"ok": False, "error": f"{target[2]}: Z_KEY, A_KEY", "at": "fixture", "revision": 3}
            view["status"] = {"phase": "error", "last_reconcile": last}
            client.statuses[target[1]] = {"phase": "error", "last_reconcile": last, "services": {target[2]: {"phase": "missing"}}}
            self.assertTrue(cd.missing_secrets(view, client.statuses[target[1]], target))
            for changed in ({"step": "secret/db"}, {"error": "unrelated failure: missing secret keys"}, {"ok": True}):
                status = copy.deepcopy(client.statuses[target[1]])
                status["last_reconcile"].update(changed)
                self.assertFalse(cd.missing_secrets(view, status, target))
            status = copy.deepcopy(client.statuses[target[1]])
            status["services"][target[2]]["phase"] = "degraded"
            self.assertFalse(cd.missing_secrets(view, status, target))
        cd.rollout(client, cd.TARGETS, report, True)
        self.assertIn("expected: missing secret keys", report["cma-am-staging"]["phase"])


class ProvenanceTests(unittest.TestCase):
    def api(self, path):
        if path == "branches/develop":
            return {"commit": {"sha": SHA}}
        if path.startswith("compare/"):
            return {"status": "identical"}
        if path.startswith("actions/workflows/"):
            return {"workflow_runs": [RUN]}
        return copy.deepcopy(RUN)

    def test_select_exact_automatic_and_manual_run(self):
        for event, sha in (({"workflow_run": RUN}, ""), ({}, ""), ({}, SHA)):
            self.assertEqual(cd.select_run(event, sha, self.api), RUN)

    def test_reject_untrusted_run_and_manual_input(self):
        for key, value in (("event", "pull_request"), ("head_branch", "topic"), ("conclusion", "failure"),
                           ("status", "in_progress"), ("head_sha", "d" * 40), ("id", True), ("run_attempt", 0),
                           ("path", ".github/workflows/other.yml"), ("head_repository", {"full_name": "fork/repo"})):
            run = dict(RUN, **{key: value})
            with self.subTest(key=key), self.assertRaises(cd.Refusal):
                cd.validate_run(run, SHA)
        with self.assertRaises(cd.Refusal):
            cd.select_run({}, "develop; echo unsafe", self.api)
        for field, replacement in (("compare/", {"status": "diverged"}), ("actions/workflows/", {"workflow_runs": []}),
                                   ("branches/", {"commit": {"sha": "d" * 40}}), ("actions/runs/", dict(RUN, run_attempt=3))):
            def changed(path, field=field, replacement=replacement):
                return replacement if path.startswith(field) else self.api(path)
            event = {"workflow_run": RUN} if field in ("branches/", "actions/runs/") else {}
            with self.subTest(field=field), self.assertRaises(cd.Refusal):
                cd.select_run(event, SHA, changed)

    def bundle(self, folder, target):
        component = target[0]
        folder.mkdir(exist_ok=True)
        archive = folder / f"cma-{component}-linux-amd64.docker.tar.gz"
        archive.write_bytes(b"opaque image fixture; Docker inspection is injected")
        metadata = {"schema": "cma-ci-native-image-v1", "component": component, "source": cd.SOURCE,
                    "sourceRevision": SHA, "platform": "linux/amd64", "archiveFormat": "docker-save+gzip",
                    "archive": archive.name, "testedReference": f"cma-{component}:ci", "imageId": IMAGE,
                    "sourceTree": "d" * 40, "archiveBytes": archive.stat().st_size,
                    "archiveSha256": hashlib.sha256(archive.read_bytes()).hexdigest(),
                    "workflow": {"repository": cd.REPO, "runId": "123", "runAttempt": "2", "event": "push", "ref": "refs/heads/develop"}}
        (folder / "metadata.json").write_text(json.dumps(metadata))
        return metadata

    def image(self, target):
        return {"Id": IMAGE, "Os": "linux", "Architecture": "amd64", "Config": {"User": f"{target[4]}:{target[4]}",
                "Labels": {"org.opencontainers.image.source": cd.SOURCE, "org.opencontainers.image.revision": SHA}}}

    def test_image_provenance_checksum_users_arch_and_labels(self):
        with tempfile.TemporaryDirectory() as directory:
            folder = Path(directory)
            for target in cd.TARGETS:
                metadata = self.bundle(folder, target)
                image = self.image(target)
                runner = Mock(return_value=json.dumps([image]))
                self.assertEqual(cd.verify_image(folder, target, RUN, runner), IMAGE)
                for key, value in (("sourceRevision", "e" * 40), ("archiveSha256", "bad"), ("archiveBytes", 0),
                                   ("sourceTree", "bad"), ("imageId", "mutable:tag"), ("workflow", {"event": "pull_request"})):
                    (folder / "metadata.json").write_text(json.dumps(dict(metadata, **{key: value})))
                    with self.subTest(key=key), self.assertRaises(cd.Refusal):
                        cd.verify_image(folder, target, RUN, runner)
                (folder / "metadata.json").write_text(json.dumps(metadata))
                for mutate in (lambda i: i.update(Architecture="arm64"), lambda i: i.update(Os="windows"),
                               lambda i: i["Config"].update(User="root"), lambda i: i["Config"].update(User="0"),
                               lambda i: i["Config"]["Labels"].update({"org.opencontainers.image.revision": "wrong"})):
                    wrong = copy.deepcopy(image)
                    mutate(wrong)
                    with self.assertRaises(cd.Refusal):
                        cd.verify_image(folder, target, RUN, Mock(return_value=json.dumps([wrong])))

    def test_download_exact_attempt_and_expired_artifacts(self):
        artifacts = [{"name": f"ci-native-image-{t[0]}-{SHA}-123-2", "expired": False} for t in cd.TARGETS]
        with tempfile.TemporaryDirectory() as directory:
            def runner(*args):
                if args[:3] == ("gh", "run", "download"):
                    target = next(t for t in cd.TARGETS if args[-1].endswith(t[0]))
                    self.bundle(Path(args[-1]), target)
                return ""
            with patch.object(cd, "verify_image", return_value=IMAGE):
                result = cd.download_images(RUN, Path(directory), runner, lambda _: {"artifacts": artifacts})
            self.assertEqual(set(result), {"web", "api", "cmaeg"})
            for broken in ([], [dict(artifacts[0], expired=True)], [artifacts[0], artifacts[0]]):
                with self.assertRaises(cd.Refusal):
                    cd.download_images(RUN, Path(directory), runner, lambda _, broken=broken: {"artifacts": broken})


class TransportTests(unittest.TestCase):
    def response(self, code, body):
        value = io.BytesIO(json.dumps(body).encode())
        value.code = code
        return value

    def test_auth_scope_if_match_and_put_body(self):
        # An in-memory sentinel never reaches a live endpoint or the repository.
        client = cd.ChronoCD("fixture")
        client.opener.open = Mock(return_value=self.response(200, app_view(cd.TARGETS[0])))
        body = cd.replacement_spec(app_view(cd.TARGETS[0]), cd.TARGETS[0], reference(cd.TARGETS[0]))
        client.request("cma-staging", method="PUT", generation=7, body=body)
        request = client.opener.open.call_args.args[0]
        self.assertEqual(request.method, "PUT")
        self.assertEqual(request.get_header("X-api-key"), "fixture")
        self.assertEqual(request.get_header("If-match"), "7")
        self.assertTrue(request.full_url.endswith("/cma-staging?org_id=internal:chrono-ai-staging"))
        self.assertEqual(json.loads(request.data), body)
        self.assertEqual(cd.NoRedirect().redirect_request(None, None, 302, "", {}, "https://invalid"), None)

    def test_real_http_roundtrip_dry_run_and_full_spec_put(self):
        state, requests = FakeCD(), []

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_args):
                return  # The production client, not server diagnostics, is under test.

            def handle_app(self):
                requests.append((self.command, self.path, self.headers.get("X-API-Key")))
                parts = self.path.split("?", 1)[0].split("/")
                is_status = parts[-1] == "status"
                app = parts[-2] if is_status else parts[-1]
                options = {"status": is_status}
                if self.command == "PUT":
                    options.update(body=json.loads(self.rfile.read(int(self.headers["Content-Length"]))),
                                   generation=int(self.headers["If-Match"]))
                response = state.request(app, method=self.command, **options)
                encoded = json.dumps(response).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(encoded)))
                self.end_headers()
                self.wfile.write(encoded)

            do_GET = handle_app
            do_PUT = handle_app

        server = HTTPServer(("127.0.0.1", 0), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            with patch.object(cd, "API", f"http://127.0.0.1:{server.server_port}/api/v1"):
                client, publisher = cd.ChronoCD("fixture"), Mock()
                cd.promote(client, {}, SHA, rows(), True, publisher=publisher)
                publisher.assert_not_called()
                self.assertEqual({method for method, _, _ in requests}, {"GET"})
                def publish(_images, _sha, report):
                    for target in cd.TARGETS:
                        report[target[1]]["image"] = reference(target)
                cd.promote(client, {}, SHA, rows(), False, publisher=publish)
                self.assertEqual(sum(method == "PUT" for method, _, _ in requests), 3)
                self.assertTrue(all(path.endswith("?org_id=internal:chrono-ai-staging") for _, path, _ in requests))
                self.assertTrue(all(key == "fixture" for _, _, key in requests))
        finally:
            server.shutdown()
            thread.join(timeout=2)
            server.server_close()

    def test_http_failures_are_redacted_and_missing_keys_are_specific(self):
        for code, body in ((403, {}), (409, {}), (401, {}), (502, {}), (422, {"error": "other"})):
            client = cd.ChronoCD("fixture")
            client.opener.open = Mock(side_effect=urllib.error.HTTPError(cd.API, code, "sensitive", {}, io.BytesIO(json.dumps(body).encode())))
            with self.subTest(code=code), self.assertRaises(cd.Refusal) as caught:
                client.request("cma-am-staging", method="PUT", body={})
            self.assertNotIn("sensitive", str(caught.exception))
        client = cd.ChronoCD("fixture")
        for app in ("cma-am-staging", "cmaeg-staging"):
            client.opener.open = Mock(return_value=self.response(422, {"error": "missing_secret_keys", "project": {"name": app}}))
            self.assertEqual(client.request(app, method="PUT", body={}), {"name": app})
        client.opener.open = Mock(return_value=self.response(422, {"error": "missing_secret_keys"}))
        with self.assertRaises(cd.Refusal):
            client.request("cma-staging", method="PUT", body={})
        client.opener.open = Mock(side_effect=urllib.error.URLError("sensitive"))
        with self.assertRaises(cd.Refusal):
            client.request("cma-staging")
        for key in ("", "key\ninvalid"):
            with self.assertRaises(cd.Refusal):
                cd.ChronoCD(key)

    def test_publish_digest_binding_and_access_failure(self):
        def runner(*args, **kwargs):
            if args[:3] == ("docker", "image", "inspect"):
                return json.dumps([{"RepoDigests": [args[-1].rsplit(":", 1)[0] + "@" + MANIFEST]}])
            if args[:3] == ("docker", "manifest", "inspect"):
                return json.dumps({"config": {"digest": IMAGE}})
            return ""
        with patch.dict(os.environ, {"GITHUB_ACTOR": "actor", "GH_TOKEN": "fixture"}):
            report = rows()
            cd.publish({t[0]: IMAGE for t in cd.TARGETS}, SHA, report, runner)
            self.assertEqual(report["cma-staging"]["image"], reference(cd.TARGETS[0]))
            def denied(*args, **kwargs):
                if args[:3] == ("docker", "image", "push"):
                    raise cd.Refusal("denied")
                return runner(*args, **kwargs)
            with self.assertRaisesRegex(cd.Refusal, "Manage Actions access"):
                cd.publish({"web": IMAGE}, SHA, rows(), denied)
            def wrong(*args, **kwargs):
                return "{}" if args[:3] == ("docker", "manifest", "inspect") else runner(*args, **kwargs)
            with self.assertRaisesRegex(cd.Refusal, "registry config"):
                cd.publish({"web": IMAGE}, SHA, rows(), wrong)

    def test_command_redaction_timeout_and_success(self):
        with patch.object(cd.subprocess, "run", return_value=Mock(returncode=0, stdout="ok")) as run, \
             patch.dict(os.environ, {"NYXID_CHRONO_CD_KEY": "fixture"}):
            self.assertEqual(cd.command("tool"), "ok")
            self.assertNotIn("NYXID_CHRONO_CD_KEY", run.call_args.kwargs["env"])
        for failure in (OSError(), subprocess.TimeoutExpired("tool", 1)):
            with patch.object(cd.subprocess, "run", side_effect=failure), self.assertRaises(cd.Refusal):
                cd.command("tool")
        with patch.object(cd.subprocess, "run", return_value=Mock(returncode=1)), self.assertRaises(cd.Refusal):
            cd.command("tool")

    def test_main_dry_run_summary_and_redacted_failures(self):
        with tempfile.TemporaryDirectory() as directory:
            event, output = Path(directory) / "event.json", Path(directory) / "summary.md"
            event.write_text("{}")
            env = {"GITHUB_REPOSITORY": cd.REPO, "GITHUB_EVENT_NAME": "workflow_dispatch", "DRY_RUN": "true",
                   "GITHUB_EVENT_PATH": str(event), "GITHUB_STEP_SUMMARY": str(output), "NYXID_CHRONO_CD_KEY": "fixture"}
            with patch.dict(os.environ, env), patch.object(cd, "select_run", return_value=RUN), \
                 patch.object(cd, "download_images", return_value={}), patch.object(cd, "ChronoCD", return_value=FakeCD()):
                self.assertEqual(cd.main(), 0)
                self.assertIn("dry run (no push or PUT)", output.read_text())
                self.assertIn(SHA, output.read_text())
                with patch.object(cd, "promote", side_effect=ValueError("sensitive")):
                    self.assertEqual(cd.main(), 1)
                with patch.object(cd, "promote", side_effect=cd.Refusal("safe diagnostic")):
                    self.assertEqual(cd.main(), 1)
                self.assertNotIn("sensitive", output.read_text())
                self.assertIn("safe diagnostic", output.read_text())


if __name__ == "__main__":
    unittest.main()
