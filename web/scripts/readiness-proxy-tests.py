#!/usr/bin/env python3
"""Exercise the shipped nginx routes against a disposable readiness upstream.

Only upstream addresses and the static root change in the application config.
This verifies the web fallback hop, not the privately managed edge controller.
"""

import http.client
import json
from pathlib import Path
import re
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
        config += f"        return 200 '{json.dumps(READY)}';\n    }}\n}}\n"
        # A different service cannot accidentally satisfy Agent Manager readiness.
        config += "server { listen 18081; return 404; }\n"
        (directory / "default.conf").write_text(config)
        (directory / "index.html").write_text("<!doctype html><title>SPA fixture</title>")
        image = re.search(r"^FROM (nginxinc/\S+) AS runtime$", (ROOT / "web/Dockerfile").read_text(), re.M)[1]
        cls.container = docker(
            "run", "--detach", "--rm", "--publish", "127.0.0.1::8080",
            "--volume", f"{directory}/default.conf:/etc/nginx/conf.d/default.conf:ro",
            "--volume", f"{directory}/index.html:/tmp/readiness-static/index.html:ro",
            image,
        )
        cls.addClassCleanup(docker, "rm", "--force", cls.container)
        cls.port = int(docker("port", cls.container, "8080/tcp").rsplit(":", 1)[1])
        deadline = time.monotonic() + 15
        while True:
            try:
                cls.get("/")
                break
            except (OSError, http.client.HTTPException):
                if time.monotonic() >= deadline:
                    raise AssertionError(docker("logs", cls.container))
                time.sleep(0.1)

    @classmethod
    def get(cls, path):
        connection = http.client.HTTPConnection("127.0.0.1", cls.port, timeout=2)
        try:
            connection.request("GET", path)
            response = connection.getresponse()
            headers = {key.lower(): value for key, value in response.getheaders()}
            return response.status, headers, response.read()
        finally:
            connection.close()

    def test_anonymous_ready_preserves_json(self):
        response = self.get("/ready")
        self.assertTrue(ready(response), response)
        self.assertEqual(response[1]["cache-control"], "no-store")
        self.assertNotIn("location", response[1])
        self.assertEqual(response[2], json.dumps(READY).encode())

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

    def test_monitor_rejects_malformed_or_incomplete_assertions(self):
        for body in [b"{", b"null", b"[]", b'{}', b'{"status":"ready"}']:
            with self.subTest(body=body):
                self.assertFalse(ready((200, {"content-type": "application/json"}, body)))


if __name__ == "__main__":
    unittest.main()
