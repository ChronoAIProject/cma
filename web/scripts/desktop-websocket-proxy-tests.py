#!/usr/bin/env python3
"""Exercise the shipped Web proxy and a synthetic upstream on an isolated network."""

import base64
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import re
import socket
import subprocess
import sys
import time
import uuid


DESKTOP = "/api/v1/agents/agt_proxy_fixture/desktop"
SOCKET = f"{DESKTOP}/ws"
ALIAS = "/api/agent-manager" + DESKTOP.removeprefix("/api")
QUERY = "?fixture=a%2Fb&sample=1&sample=2"
ORIGIN = "https://web-fixture.invalid"
HOST = "web-fixture.invalid"
COOKIE = "fixture_login=accepted"
AUTHORIZATION = "Bearer fixture-only"
KEY = base64.b64encode(b"websocketfixture").decode()
RFB = b"RFB 003.008\n"
FRAME = bytes((0x82, len(RFB))) + RFB
CLIENT_ADDRESS = "192.0.2.7"


def accept_key(key):
    material = (key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode()
    return base64.b64encode(hashlib.sha1(material).digest()).decode()


class Upstream(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_args):
        pass

    def do_GET(self):
        upgrade = self.headers.get("Upgrade", "")
        connection = self.headers.get("Connection", "")
        refusal = ""
        status = 200
        if self.path.split("?", 1)[0] == SOCKET:
            if (upgrade, connection) != ("websocket", "upgrade"):
                status, refusal = 400, "upgrade"
            elif self.headers.get("Origin") != ORIGIN:
                status, refusal = 403, "origin"
            elif (self.headers.get("Cookie"), self.headers.get("Authorization")) != (COOKIE, AUTHORIZATION):
                status, refusal = 401, "authorization"
            else:
                status = 101
        self.send_response(status)
        self.send_header("X-Fixture-Uri", self.path)
        self.send_header("X-Fixture-Upgrade", upgrade)
        self.send_header("X-Fixture-Connection", connection)
        self.send_header("X-Fixture-Host", self.headers.get("Host", ""))
        self.send_header("X-Fixture-Forwarded-For", self.headers.get("X-Forwarded-For", ""))
        self.send_header("X-Fixture-Forwarded-Proto", self.headers.get("X-Forwarded-Proto", ""))
        if refusal:
            self.send_header("X-Fixture-Refusal", refusal)
        if status == 101:
            self.send_header("Upgrade", "websocket")
            self.send_header("Connection", "Upgrade")
            self.send_header("Sec-WebSocket-Accept", accept_key(self.headers["Sec-WebSocket-Key"]))
            self.send_header("X-Fixture-WebSocket-Version", self.headers["Sec-WebSocket-Version"])
        else:
            self.send_header("Content-Length", "0")
        self.end_headers()
        if status == 101:
            self.wfile.write(FRAME)
            self.wfile.flush()
        self.close_connection = True


def request(path, headers):
    with socket.create_connection(("web-proxy", 8080), timeout=3) as client:
        lines = [f"GET {path} HTTP/1.1", f"Host: {HOST}", f"X-Forwarded-For: {CLIENT_ADDRESS}"]
        lines.extend(f"{name}: {value}" for name, value in headers.items() if value is not None)
        client.sendall(("\r\n".join(lines) + "\r\n\r\n").encode("ascii"))
        with client.makefile("rb") as wire:
            status = int(wire.readline().split()[1])
            response = {}
            for line in wire:
                if line == b"\r\n":
                    break
                name, value = line.decode("ascii").split(":", 1)
                response[name.lower()] = value.strip()
            frame = wire.read(len(FRAME)) if status == 101 else b""
            return status, response, frame


def test_proxy():
    deadline = time.monotonic() + 10
    while True:
        try:
            if request("/ready", {})[0] == 200:
                break
        except OSError:
            pass
        if time.monotonic() >= deadline:
            raise AssertionError("Web proxy did not become ready")
        time.sleep(0.1)

    handshake = {
        "Upgrade": "WebSocket",
        "Connection": "keep-alive,\tUPgrade , keep-alive",
        "Sec-WebSocket-Version": "13",
        "Sec-WebSocket-Key": KEY,
        "Origin": ORIGIN,
        "Cookie": COOKIE,
        "Authorization": AUTHORIZATION,
    }

    def check(path, headers, expected, upstream, upgraded=False, refusal=None):
        status, observed, frame = request(path, headers)
        assert status == expected, (path, expected, status, observed)
        assert observed["x-fixture-uri"] == upstream, (path, observed)
        assert observed["x-fixture-upgrade"] == ("websocket" if upgraded else ""), observed
        assert observed["x-fixture-connection"] in (("upgrade",) if upgraded else ("", "close")), observed
        assert observed["x-fixture-host"] == HOST, observed
        forwarded = observed["x-fixture-forwarded-for"].split(", ")
        assert len(forwarded) == 2 and forwarded[0] == CLIENT_ADDRESS, forwarded
        assert observed["x-fixture-forwarded-proto"] == "http", observed
        assert observed.get("x-fixture-refusal") == refusal, observed
        if expected == 101:
            assert observed["sec-websocket-accept"] == accept_key(KEY), observed
            assert observed["x-fixture-websocket-version"] == "13", observed
            assert frame == FRAME, frame

    for desktop in (DESKTOP, ALIAS):
        path = f"{desktop}/ws{QUERY}"
        check(path, handshake, 101, SOCKET + QUERY, upgraded=True)
        check(path, {**handshake, "Origin": "https://other-fixture.invalid"}, 403,
              SOCKET + QUERY, upgraded=True, refusal="origin")
        check(path, {**handshake, "Authorization": None}, 401,
              SOCKET + QUERY, upgraded=True, refusal="authorization")
        for missing_or_invalid in (
            {"Upgrade": None},
            {"Connection": None},
            {"Upgrade": "websocket-fake"},
            {"Upgrade": "websocket, h2c"},
            {"Connection": "keep-alive, x-upgrade"},
            {"Connection": "upgrade-extra, keep-alive"},
            {"Connection": "keep-alive upgrade"},
        ):
            check(path, {**handshake, **missing_or_invalid}, 400, SOCKET + QUERY, refusal="upgrade")
        check(desktop + QUERY, {}, 200, DESKTOP + QUERY)
        print(f"{desktop}/ws passed 101, binary RFB and upgrade, Origin and auth refusals")

    check("/api/v1/agents" + QUERY, handshake, 200, "/api/v1/agents" + QUERY)
    check("/api/agent-manager/v1/agents" + QUERY, handshake, 200, "/api/v1/agents" + QUERY)
    check(SOCKET + "/extra", handshake, 200, SOCKET + "/extra")
    print("Discovery and unrelated API routes passed without upgrade forwarding")


def docker(*arguments):
    return subprocess.check_output(["docker", *arguments], text=True, timeout=45).strip()


def run_fixture():
    script = Path(__file__).resolve()
    web = script.parents[1]
    image = re.search(r"^FROM (nginxinc/\S+) AS runtime$", (web / "Dockerfile").read_text(), re.M)[1]
    network = f"web-desktop-proxy-{uuid.uuid4().hex}"
    backend = f"{network}-backend"
    proxy = f"{network}-nginx"
    runner = f"{network}-runner"
    script_mount = f"type=bind,source={script},target=/fixture.py,readonly"
    config_mount = f"type=bind,source={web / 'nginx.conf'},target=/etc/nginx/conf.d/default.conf,readonly"
    python_image = "python:3.12-alpine"
    docker("network", "create", "--internal", network)
    containers = []
    try:
        docker("run", "--detach", "--pull=never", "--name", backend, "--network", network,
               "--network-alias", "cma-agent-manager", "--network-alias", "cma-event-gateway",
               "--network-alias", "cma-image-manager", "--mount", script_mount,
               python_image, "python", "/fixture.py", "upstream")
        containers.append(backend)
        docker("run", "--detach", "--pull=never", "--name", proxy, "--network", network,
               "--network-alias", "web-proxy", "--mount", config_mount, image)
        containers.append(proxy)
        docker("exec", proxy, "nginx", "-t")
        containers.append(runner)
        print(docker("run", "--pull=never", "--name", runner, "--network", network,
                     "--mount", script_mount, python_image, "python", "/fixture.py", "probe"))
    finally:
        for container in reversed(containers):
            subprocess.run(["docker", "rm", "--force", container], stdout=subprocess.DEVNULL,
                           stderr=subprocess.DEVNULL, timeout=15, check=False)
        docker("network", "rm", network)


if __name__ == "__main__":
    if sys.argv[1:] == ["upstream"]:
        ThreadingHTTPServer(("0.0.0.0", 80), Upstream).serve_forever()
    elif sys.argv[1:] == ["probe"]:
        test_proxy()
    else:
        run_fixture()
