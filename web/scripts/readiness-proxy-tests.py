#!/usr/bin/env python3
"""Exercise the shipped nginx routes against a disposable readiness upstream.

Only upstream addresses and the static root change in the application config.
This verifies the web fallback hop, not the privately managed edge controller.
The desktop socket route policy is checked statically from the same file.
"""

import http.client
import json
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import time
import unittest


ROOT = Path(__file__).resolve().parents[2]
READY = {"status": "ready", "reason": None, "dependency": {"reason": None}}
STORE_DOWN = {
    "status": "not-ready", "reason": "store", "dependency": {"reason": "unavailable"}
}
WORKERS_DOWN = {
    "status": "not-ready", "reason": "workers", "dependency": {"reason": None}
}
STREAM_OPEN = b"event: open\ndata: started\n\n"
STREAM_END = b"event: completed\ndata: finished\n\n"
STREAM_BODY = STREAM_OPEN + b"data: " + b"x" * 4096 + b"\n\n" + STREAM_END


def docker(*args):
    return subprocess.check_output(["docker", *args], text=True, timeout=180).strip()


def ready(response):
    """The monitor contract also rejects HTML 200, redirects and malformed JSON."""
    status, headers, body = response
    if status != 200 or headers.get("content-type", "").split(";", 1)[0] != "application/json":
        return False
    try:
        value = json.loads(body)
    except (ValueError, UnicodeDecodeError):
        return False
    return (
        isinstance(value, dict)
        and value.get("status") == "ready"
        and "reason" in value and value["reason"] is None
        and isinstance(value.get("dependency"), dict)
        and "reason" in value["dependency"] and value["dependency"]["reason"] is None
    )


class WebReadiness(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.files = tempfile.TemporaryDirectory(prefix="web-readiness-")
        cls.addClassCleanup(cls.files.cleanup)
        directory = Path(cls.files.name)
        config = (ROOT / "web/nginx.conf").read_text()
        config = re.sub(
            r"http://cma-[a-z-]+:80",
            lambda match: "http://127.0.0.1:" + (
                "18080" if match[0] == "http://cma-agent-manager:80" else "18081"
            ),
            config,
        )
        config = config.replace("/usr/share/nginx/html", "/tmp/readiness-static")
        # Credentials would make the fake backend fail, proving anonymous use.
        config += """
server {
    listen 18080;
    default_type application/json;
    location = /ready {
        if ($http_authorization != "") { return 401; }
        if ($http_cookie != "") { return 401; }
"""
        config += f"        if ($arg_state = store) {{ return 503 '{json.dumps(STORE_DOWN)}'; }}\n"
        config += f"        if ($arg_state = workers) {{ return 503 '{json.dumps(WORKERS_DOWN)}'; }}\n"
        config += f"        return 200 '{json.dumps(READY)}';\n    }}\n"
        config += """    location ~ ^/api/v1/documents(?:/|$) {
        client_max_body_size 64m;
        add_header Cache-Control 'private, no-store' always;
        add_header Referrer-Policy no-referrer always;
        if ($arg_limit = 0) { return 400 '{"code":"invalid_query"}'; }
        if ($arg_reject = true) {
            add_header Cache-Control 'private, no-store' always;
            add_header Referrer-Policy no-referrer always;
            add_header Referrer-Policy strict-origin-when-cross-origin always;
            return 401 '{"code":"authentication_required"}';
        }
        return 202 '{"reached":"agent-manager"}';
    }
    location = /auth/service-authorization/callback {
        add_header Cache-Control 'private, no-store' always;
        add_header Referrer-Policy no-referrer always;
        if ($arg_state = invalid) { return 400 '{"code":"invalid_request"}'; }
        if ($arg_state = unknown) { return 409 '{"code":"authorization_changed"}'; }
        return 200 '{"code":"cancelled"}';
    }
    location = /auth/session {
        return 401 '{"code":"authentication_required"}';
    }
"""
        config += """    location = /api/drain-stream {
        default_type text/event-stream;
        limit_rate 1024;
        alias /tmp/readiness-static/drain-stream;
    }
"""
        # Any API body the web proxy forwards reaches the fake Agent Manager.
        config += """    location /api/ {
        client_max_body_size 64m;
        return 202 '{"reached":"agent-manager"}';
    }
}
"""
        # A different service cannot accidentally satisfy Agent Manager readiness.
        config += "server { listen 18081; return 404; }\n"
        (directory / "default.conf").write_text(config)
        (directory / "index.html").write_text("<!doctype html><title>SPA fixture</title>")
        (directory / "drain-stream").write_bytes(STREAM_BODY)
        for name in ("web-clear-drain.sh", "web-prestop.sh"):
            script = directory / name
            shutil.copyfile(ROOT / "web/scripts" / name, script)
            script.chmod(0o755)
        image = re.search(r"^FROM (nginxinc/\S+) AS runtime$", (ROOT / "web/Dockerfile").read_text(), re.M)[1]
        cls.container = docker(
            "run", "--detach", "--publish", "127.0.0.1::8080",
            "--volume", f"{directory}/default.conf:/etc/nginx/conf.d/default.conf:ro",
            "--volume", f"{directory}/index.html:/tmp/readiness-static/index.html:ro",
            "--volume", f"{directory}/drain-stream:/tmp/readiness-static/drain-stream:ro",
            "--volume", f"{directory}/web-clear-drain.sh:/docker-entrypoint.d/40-clear-web-drain.sh:ro",
            "--volume", f"{directory}/web-prestop.sh:/usr/local/bin/cma-web-prestop:ro",
            image,
        )
        cls.addClassCleanup(docker, "rm", "--force", cls.container)
        cls.port = int(docker("port", cls.container, "8080/tcp").rsplit(":", 1)[1])
        cls.wait_for_status("/", 200)

    @classmethod
    def wait_for_status(cls, path, status):
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            try:
                response = cls.get(path)
                if response[0] == status:
                    return response
            except (OSError, http.client.HTTPException):
                pass
            time.sleep(0.1)
        raise AssertionError(docker("logs", cls.container))

    @classmethod
    def get(cls, path):
        return cls.request("GET", path)

    @classmethod
    def post(cls, method, path, size):
        return cls.request(method, path, body=b"x" * size,
                           headers={"Content-Type": "application/json"})

    @classmethod
    def request(cls, method, path, body=None, headers=None):
        connection = http.client.HTTPConnection("127.0.0.1", cls.port, timeout=10)
        try:
            connection.request(method, path, body=body, headers=headers or {})
            response = connection.getresponse()
            headers = {key.lower(): value for key, value in response.getheaders()}
            headers["referrer-policy"] = response.headers.get_all("Referrer-Policy", [])
            return response.status, headers, response.read()
        finally:
            connection.close()

    def test_anonymous_ready_preserves_json(self):
        response = self.get("/ready")
        self.assertTrue(ready(response), response)
        self.assertEqual(response[1]["cache-control"], "no-store")
        self.assertNotIn("location", response[1])
        self.assertEqual(response[2], json.dumps(READY).encode())

    def test_web_prestop_withdraws_readiness_while_proxy_stream_completes(self):
        status, headers, body = self.get("/web-readyz")
        self.assertEqual((status, headers["cache-control"], body), (200, "no-store", b"ready\n"))
        connection = http.client.HTTPConnection("127.0.0.1", self.port, timeout=10)
        hook = None
        try:
            connection.request("GET", "/api/agent-manager/drain-stream")
            stream = connection.getresponse()
            self.assertEqual(stream.status, 200)
            self.assertEqual(stream.read(len(STREAM_OPEN)), STREAM_OPEN)
            hook = subprocess.Popen(["docker", "exec", self.container, "/usr/local/bin/cma-web-prestop"])
            _, headers, body = self.wait_for_status("/web-readyz", 503)
            self.assertEqual((headers["cache-control"], body), ("no-store", b"draining\n"))
            self.assertEqual(self.get("/")[0], 200)
            self.assertTrue(ready(self.get("/ready")))
            self.assertEqual(stream.read(), STREAM_BODY[len(STREAM_OPEN):])
            self.assertIsNone(hook.poll(), "preStop returned before the stream completed")
        finally:
            connection.close()
            docker("restart", "--timeout", "5", self.container)
            if hook is not None:
                hook.wait(timeout=10)
            type(self).port = int(docker("port", self.container, "8080/tcp").rsplit(":", 1)[1])
        _, headers, body = self.wait_for_status("/web-readyz", 200)
        self.assertEqual((headers["cache-control"], body), ("no-store", b"ready\n"))

    def test_not_ready_preserves_503_and_body(self):
        for state, body in [("store", STORE_DOWN), ("workers", WORKERS_DOWN)]:
            with self.subTest(state=state):
                response = self.get(f"/ready?state={state}")
                self.assertEqual(response[0], 503)
                self.assertEqual(response[1]["content-type"], "application/json")
                self.assertEqual(response[1]["cache-control"], "no-store")
                self.assertNotIn("location", response[1])
                self.assertEqual(response[2], json.dumps(body).encode())
                self.assertFalse(ready(response))

    def test_spa_fallback_cannot_satisfy_readiness(self):
        for path in ["/", "/ready/", "/ready/nested", "/ready-other"]:
            with self.subTest(path=path):
                response = self.get(path)
                self.assertEqual(response[0], 200)
                self.assertIn(b"SPA fixture", response[2])
                self.assertFalse(ready(response))

    def test_service_authorization_callback_has_only_no_referrer(self):
        for state, status, code in [
            ("known", 200, "cancelled"),
            ("invalid", 400, "invalid_request"),
            ("unknown", 409, "authorization_changed"),
        ]:
            with self.subTest(state=state):
                actual_status, headers, body = self.get(
                    f"/auth/service-authorization/callback?state={state}&error=access_denied"
                )
                self.assertEqual(actual_status, status)
                self.assertEqual(json.loads(body), {"code": code})
                self.assertEqual(headers["referrer-policy"], ["no-referrer"])
                self.assertEqual(headers["cache-control"], "private, no-store")

        status, headers, _ = self.get("/auth/session")
        self.assertEqual(status, 401)
        self.assertEqual(headers["referrer-policy"], ["strict-origin-when-cross-origin"])

    def test_legal_document_and_artifact_bodies_reach_the_agent_manager(self):
        # 1 MiB of Markdown or decoded artifact bytes is legal; JSON and base64
        # expansion must not turn it into an untyped HTML 413 at the proxy.
        mib = 1024 * 1024
        for method, path, size in [
            ("PATCH", "/api/v1/documents/doc-a", 6 * mib + 16 * 1024),
            ("POST", "/api/v1/documents", 6 * mib + 16 * 1024),
            ("POST", "/api/agent-manager/v1/documents", 6 * mib + 16 * 1024),
            ("POST", "/api/v1/agents/agent-a/artifacts", 2 * mib),
            ("POST", "/api/agent-manager/v1/agents/agent-a/artifacts", 2 * mib),
        ]:
            with self.subTest(path=path):
                status, _, body = self.post(method, path, size)
                self.assertEqual((status, body), (202, b'{"reached":"agent-manager"}'))
        # The raised bounds are route-scoped; other API routes keep nginx's default.
        status, _, _ = self.post("PATCH", "/api/v1/agents/agent-a/settings", mib + 1)
        self.assertEqual(status, 413)

    def test_document_response_keeps_one_strict_referrer_policy(self):
        for prefix in ("/api/v1/documents", "/api/agent-manager/v1/documents"):
            for suffix, expected in [
                ("/doc-a", 202),
                ("/doc-a/source", 202),
                ("?limit=0", 400),
                ("?reject=true", 401),
                ("/doc-a/source?reject=true", 401),
            ]:
                with self.subTest(path=prefix + suffix):
                    status, headers, _ = self.get(prefix + suffix)
                    self.assertEqual(status, expected)
                    self.assertEqual(headers["referrer-policy"], ["no-referrer"])
                    self.assertEqual(headers["cache-control"], "private, no-store")
        self.assertEqual(
            self.get("/api/v1/agents")[1]["referrer-policy"],
            ["strict-origin-when-cross-origin"],
        )

    def test_nginx_document_error_has_one_strict_referrer_policy(self):
        for prefix in ("/api/v1/documents", "/api/agent-manager/v1/documents"):
            for suffix, size in [("", 7 * 1024 * 1024 + 1), ("/doc-a/source", 1024 * 1024 + 1)]:
                with self.subTest(path=prefix + suffix):
                    status, headers, _ = self.post("POST", prefix + suffix, size)
                    self.assertEqual(status, 413)
                    self.assertEqual(headers["referrer-policy"], ["no-referrer"])

    def test_monitor_rejects_malformed_or_incomplete_assertions(self):
        for body in [b"{", b"null", b"[]", b'{}', b'{"status":"ready"}']:
            with self.subTest(body=body):
                self.assertFalse(ready((200, {"content-type": "application/json"}, body)))


class DesktopSocketRoutes(unittest.TestCase):
    """Static policy for both browser desktop socket routes; no container needed."""

    # Agent Manager closes a desktop relay after 30 minutes. An idle RFB stream can
    # send nothing for longer than nginx's 60 s default, so the proxy must not cut it.
    RELAY_LIFETIME_SECONDS = 30 * 60

    def test_both_desktop_socket_routes_upgrade_and_stay_open_while_idle(self):
        config = (ROOT / "web/nginx.conf").read_text()
        blocks = re.findall(r"location ~ (\S*/desktop/ws\$) \{(.*?)\n    \}", config, re.S)
        self.assertEqual(
            [route for route, _ in blocks],
            [r"^/api/v1/agents/[^/]+/desktop/ws$",
             r"^/api/agent-manager/v1/agents/[^/]+/desktop/ws$"],
        )
        for route, body in blocks:
            with self.subTest(route=route):
                self.assertIn("proxy_set_header Upgrade $desktop_websocket_upgrade;", body)
                self.assertIn("proxy_set_header Connection $desktop_websocket_connection;", body)
                self.assertIn("proxy_buffering off;", body)
                timeout = re.search(r"proxy_read_timeout (\d+)s;", body)
                self.assertIsNotNone(timeout, "the 60 s default would cut an idle desktop")
                self.assertGreaterEqual(int(timeout[1]), self.RELAY_LIFETIME_SECONDS)


if __name__ == "__main__":
    unittest.main()
