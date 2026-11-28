import copy
from hashlib import sha256
import json
import unittest
from unittest.mock import Mock

from staging_cd_common import Refusal
from staging_cd_web_rollout_preflight import HOOK_PATH, HOOK_SOURCE, NGINX_PATH, PROBE_PATHS, READY_PATH, verify


IMAGE = "ghcr.io/chronoaiproject/cma-web@sha256:" + "a" * 64
SOURCE = "b" * 40
VERSION = "0.22.70"
EDGE = "https://web.example.test"
DRAINING = "http://127.0.0.1:8080"
NGINX = "reviewed nginx mount\n"
HTML = b"<!doctype html><title>CMA</title>"
JAVASCRIPT = b"export const ready = true;\n"
ASSET = "/assets/entry-A1.js"
SENTINEL = {"path": ASSET, "sha256": sha256(JAVASCRIPT).hexdigest(), "html_sha256": sha256(HTML).hexdigest()}


class WebRolloutPreflightTests(unittest.TestCase):
    def setUp(self):
        self.service = {
            "image": IMAGE,
            "health_path": READY_PATH,
            "probes": {name: {"path": path} for name, path in PROBE_PATHS.items()},
            "termination_grace_seconds": 1050,
            "pre_stop": {"exec": [HOOK_PATH]},
            "files": [{"path": NGINX_PATH, "content": NGINX}],
        }
        identity = (200, "application/json", set(), json.dumps({"version": VERSION, "revision": SOURCE, "local": False}).encode())
        asset = (200, "text/javascript", {"public", "immutable"}, JAVASCRIPT)
        self.responses = {
            (EDGE, READY_PATH): (200, "text/plain", {"no-store"}, b"ready\n"),
            (EDGE, "/build-info.json"): identity,
            (EDGE, "/"): (200, "text/html", {"no-store"}, HTML),
            (EDGE, ASSET): asset,
            (DRAINING, READY_PATH): (503, "text/plain", {"no-store"}, b"draining\n"),
            (DRAINING, "/build-info.json"): identity,
            (DRAINING, ASSET): asset,
        }
        self.fetch = Mock(side_effect=lambda origin, path: self.responses[origin, path])

    def run_preflight(self):
        verify(self.service, IMAGE, sha256(NGINX.encode()).hexdigest(), VERSION, SOURCE, SENTINEL,
               EDGE, DRAINING, fetch=self.fetch)

    def test_reads_exact_identity_and_serves_the_asset_during_drain(self):
        self.run_preflight()
        self.assertEqual([call.args for call in self.fetch.call_args_list], list(self.responses))

    def test_legacy_image_can_use_the_reviewed_mounted_hook(self):
        self.service["pre_stop"] = {"exec": ["/bin/sh", HOOK_PATH]}
        self.service["files"].append({"path": HOOK_PATH, "content": HOOK_SOURCE.read_text()})
        self.run_preflight()
        self.service["files"][-1]["content"] = "#!/bin/sh\nexit 0\n"
        with self.assertRaisesRegex(Refusal, "Mounted Web preStop differs"):
            self.run_preflight()

    def test_preflight_requires_bound_image_mount_and_effective_probe_settings(self):
        service = copy.deepcopy(self.service)
        for field, value, reason in (
            ("pre_stop", None, "preStop executable is missing"),
            ("health_path", "/", "Web readiness, startup or liveness path differs"),
            ("probes", {"readiness": {"path": READY_PATH}}, "Web readiness, startup or liveness path differs"),
            ("termination_grace_seconds", 30, "Web termination grace differs"),
            ("files", [{"path": NGINX_PATH, "content": "unreviewed mount"}], "Effective Nginx mount differs"),
            ("image", IMAGE.replace("a" * 64, "c" * 64), "Effective Web service image differs"),
        ):
            with self.subTest(field=field):
                self.service = service | {field: value}
                with self.assertRaisesRegex(Refusal, reason):
                    self.run_preflight()
        self.fetch.assert_not_called()

    def test_html_200_at_readiness_must_fail(self):
        self.responses[EDGE, READY_PATH] = (200, "text/html", {"no-store"}, HTML)
        with self.assertRaisesRegex(Refusal, "Web readiness response differs"):
            self.run_preflight()
        self.fetch.assert_called_once_with(EDGE, READY_PATH)

    def test_first_edge_asset_failure_is_retained_without_retry(self):
        self.responses[EDGE, ASSET] = (503, "text/plain", set(), b"connection timeout")
        with self.assertRaisesRegex(Refusal, "Web asset differs"):
            self.run_preflight()
        self.assertEqual(self.fetch.call_count, 4)

    def test_edge_failure_cannot_pass_as_draining_readiness(self):
        self.responses[DRAINING, READY_PATH] = (503, "text/plain", {"no-store"}, b"connection timeout")
        with self.assertRaisesRegex(Refusal, "Web readiness response differs"):
            self.run_preflight()

    def test_draining_must_keep_serving_the_exact_asset_bytes(self):
        self.responses[DRAINING, ASSET] = (200, "text/javascript", {"public", "immutable"}, b"wrong asset")
        with self.assertRaisesRegex(Refusal, "Web asset differs"):
            self.run_preflight()
