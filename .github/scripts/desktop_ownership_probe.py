"""Protected transport for isolated image probes, with synthetic claim authority.

This fixture does not qualify Mongo admission or production control planes.
"""
import base64
import hashlib
import http.client
import json
import os
from pathlib import Path
import socket
import threading
import time
import uuid


class ProtectedDesktop:
    def __init__(self, address, token, sandbox, floor):
        if address[0] != "127.0.0.1":
            raise ValueError("desktop image probe requires loopback")
        self.address, self.token, self.sandbox = address, token, sandbox
        self.socket = None
        self.owner = None
        self.failure = None
        self.stopped = threading.Event()
        self.worker = None
        self.deadline = 0
        self.epoch = self.next_epoch(floor)
        try:
            self.confirm()
            self.upgrade()
            self.worker = threading.Thread(target=self.renew, daemon=True)
            self.worker.start()
        except Exception:
            self.close()
            raise

    @classmethod
    def from_env(cls):
        sandbox = os.environ["CHRONO_SANDBOX_ID"]
        root = Path(os.environ.get("CHRONO_RUNTIME_STATE_ROOT", "/var/lib/cma-runtime/" + sandbox))
        return cls(("127.0.0.1", 9090), os.environ["CMA_DESKTOP_TEST_TOKEN"],
                   sandbox, root / "desktop-owner/floor.json")

    def next_epoch(self, floor):
        try:
            record = json.loads(Path(floor).read_text())
        except FileNotFoundError:
            return 1
        assert record["version"] == 1 and record["retired"] is True, "probe floor is not retired"
        assert record["owner"]["sandbox_id"] == self.sandbox, "probe floor Sandbox mismatch"
        epoch = record["owner"]["epoch"]
        assert type(epoch) is int and 0 < epoch < 2**63 - 1, "invalid probe floor epoch"
        return epoch + 1

    def post(self, action, body):
        connection = http.client.HTTPConnection(*self.address, timeout=2)
        try:
            connection.request("POST", "/runtime/desktop/ownership/" + action,
                               json.dumps(body), {"content-type": "application/json",
                                                  "x-cma-runtime-token": self.token})
            response = connection.getresponse()
            assert response.status == 200, "desktop ownership " + action + " refused"
            data = response.read(8193)
            assert len(data) <= 8192, "oversized desktop ownership response"
            return json.loads(data)
        finally:
            connection.close()

    def confirm(self):
        started = time.monotonic()
        previous = self.owner
        challenge = self.post("challenge", {"sandbox_id": self.sandbox,
                              "runtime_id": "image-probe", "previous": previous})
        nonce, valid = challenge["nonce"], challenge["valid_for_ms"]
        assert isinstance(nonce, str) and nonce.strip() and len(nonce) <= 128
        assert type(valid) is int and 0 < valid <= 10000
        deadline = started + valid / 1000
        assert time.monotonic() < deadline, "desktop challenge expired"
        issued = max(time.time_ns() // 1_000_000, (previous or {}).get("issued_at_ms", 0) + 1)
        owner = dict(previous) if previous else {
            "sandbox_id": self.sandbox, "runtime_id": "image-probe", "boot_id": None,
            "session_id": str(uuid.uuid4()), "epoch": self.epoch,
        }
        owner.update(issued_at_ms=issued, expires_at_ms=issued + 15000)
        # Preserve the exact candidate for revoke even if its acknowledgement is lost.
        self.owner = owner
        acknowledgement = self.post("confirm", {"ownership": owner, "nonce": nonce})
        assert acknowledgement == {"ownership": owner}, "desktop ownership acknowledgement mismatch"
        assert time.monotonic() < deadline, "desktop confirmation expired"
        self.deadline = deadline

    def upgrade(self):
        self.socket = socket.create_connection(self.address, timeout=2)
        key = base64.b64encode(os.urandom(16)).decode()
        owner = json.dumps(self.owner, separators=(",", ":"))
        self.socket.sendall(("GET /runtime/desktop HTTP/1.1\r\nHost: localhost\r\n"
            "Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\n"
            f"Sec-WebSocket-Key: {key}\r\nx-cma-runtime-token: {self.token}\r\n"
            f"x-cma-desktop-owner: {owner}\r\n\r\n").encode())
        headers = bytearray()
        while not headers.endswith(b"\r\n\r\n"):
            chunk = self.socket.recv(1)
            assert chunk and len(headers) < 16384, "invalid desktop handshake"
            headers.extend(chunk)
        assert headers.startswith(b"HTTP/1.1 101 "), "protected desktop upgrade refused"
        expected = base64.b64encode(hashlib.sha1(
            (key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode()).digest())
        fields = dict(line.split(b":", 1) for line in bytes(headers).split(b"\r\n")[1:] if b":" in line)
        assert {k.lower(): v.strip() for k, v in fields.items()}.get(b"sec-websocket-accept") == expected
        assert time.monotonic() < self.deadline, "desktop upgrade expired"
        self.socket.settimeout(10)

    def renew(self):
        try:
            while not self.stopped.wait(2):
                assert time.monotonic() < self.deadline, "desktop renewal expired"
                self.confirm()
        except Exception:
            self.failure = "desktop ownership renewal failed"
            self.disconnect()

    def disconnect(self):
        if self.socket:
            try:
                self.socket.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
            self.socket.close()

    def close(self):
        self.stopped.set()
        try:
            if self.worker:
                self.worker.join(timeout=5)
                assert not self.worker.is_alive(), "desktop renewal did not stop"
        finally:
            self.disconnect()
        if self.owner:
            owner, self.owner = self.owner, None
            acknowledgement = self.post("revoke", owner)
            assert acknowledgement == {"ownership": owner}, "desktop revoke mismatch"
        assert self.failure is None, self.failure
