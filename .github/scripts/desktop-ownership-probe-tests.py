"""Check receiver admission, retained floors and failure cleanup in image probes."""
import base64
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import importlib.util
import json
from pathlib import Path
import secrets
import tempfile
import threading
import time
import unittest
from unittest.mock import patch

from desktop_ownership_probe import ProtectedDesktop


class Receiver(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_POST(self):
        fixture = self.server.fixture
        body = json.loads(self.rfile.read(int(self.headers['content-length'])))
        action = self.path.rsplit('/', 1)[-1]
        fixture.calls.append((action, body))
        assert self.headers['x-cma-runtime-token'] == fixture.token
        if action == 'challenge':
            response = {'nonce': 'synthetic-challenge', 'valid_for_ms': fixture.valid}
        elif action == 'confirm':
            fixture.owner = body['ownership']
            response = {'ownership': fixture.owner if not fixture.bad_ack else {}}
        else:
            assert action == 'revoke' and body == fixture.owner
            response = {'ownership': body}
        self.send_response(503 if fixture.fail == action else 200)
        self.end_headers()
        self.wfile.write(json.dumps(response).encode())

    def do_GET(self):
        fixture = self.server.fixture
        fixture.upgrades += 1
        assert self.path == '/runtime/desktop'
        assert self.headers['x-cma-runtime-token'] == fixture.token
        assert json.loads(self.headers['x-cma-desktop-owner']) == fixture.owner
        self.protocol_version = 'HTTP/1.1'
        self.send_response(400 if fixture.fail == 'upgrade' else 101)
        self.send_header('Upgrade', 'websocket')
        self.send_header('Connection', 'Upgrade')
        key = self.headers['sec-websocket-key']
        digest = hashlib.sha1((key + '258EAFA5-E914-47DA-95CA-C5AB0DC85B11').encode()).digest()
        self.send_header('Sec-WebSocket-Accept', base64.b64encode(digest).decode())
        self.end_headers()
        self.wfile.flush()
        self.connection.settimeout(5)
        try:
            while self.rfile.read(1):
                pass
        except (OSError, TimeoutError):
            pass
        self.close_connection = True


class Probe(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.floor = Path(self.temp.name) / 'floor.json'
        self.calls, self.upgrades = [], 0
        self.valid, self.fail, self.bad_ack = 10000, None, False
        self.token = secrets.token_hex(32)
        self.owner = None
        self.server = ThreadingHTTPServer(('127.0.0.1', 0), Receiver)
        self.server.fixture = self
        self.thread = threading.Thread(target=self.server.serve_forever)
        self.thread.start()
        self.addCleanup(self.stop_server)

    def stop_server(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=2)
        self.assertFalse(self.thread.is_alive())

    def open(self):
        return ProtectedDesktop(self.server.server_address, self.token, 'fixture-sandbox', self.floor)

    def retained(self, epoch=7, retired=True):
        self.floor.write_text(json.dumps({'version': 1, 'retired': retired,
            'owner': {'sandbox_id': 'fixture-sandbox', 'epoch': epoch}}))

    def test_protected_upgrade_and_exact_revoke(self):
        probe = self.open()
        owner = probe.owner.copy()
        probe.close()
        self.assertEqual([name for name, _ in self.calls], ['challenge', 'confirm', 'revoke'])
        self.assertIsNone(self.calls[0][1]['previous'])
        self.assertEqual(self.calls[-1][1], owner)
        self.assertEqual(self.upgrades, 1)
        self.assertFalse(probe.worker.is_alive())

    def test_retained_floor_advances_without_rewriting_it(self):
        self.retained()
        before = self.floor.read_bytes()
        probe = self.open()
        try:
            self.assertEqual(probe.owner['epoch'], 8)
            self.assertEqual(self.floor.read_bytes(), before)
        finally:
            probe.close()

    def test_live_or_invalid_floor_refuses_before_network(self):
        for epoch, retired in [(7, False), (0, True), (2**63 - 1, True)]:
            self.retained(epoch, retired)
            with self.assertRaises(AssertionError):
                self.open()
        self.floor.write_text('{')
        with self.assertRaises(ValueError):
            self.open()
        self.assertEqual(self.calls, [])

    def test_confirmation_mismatch_revokes_without_upgrade(self):
        self.bad_ack = True
        with self.assertRaisesRegex(AssertionError, 'acknowledgement mismatch'):
            self.open()
        self.assertEqual(self.upgrades, 0)
        self.assertEqual(self.calls[-1], ('revoke', self.owner))

    def test_refused_challenge_never_falls_back_to_ownerless(self):
        self.fail = 'challenge'
        with self.assertRaisesRegex(AssertionError, 'challenge refused'):
            self.open()
        self.assertEqual(self.upgrades, 0)
        self.assertEqual(len(self.calls), 1)

    def test_expired_challenge_never_confirms(self):
        self.valid = 1
        with patch('desktop_ownership_probe.time.monotonic', side_effect=[1, 2]), \
                self.assertRaisesRegex(AssertionError, 'challenge expired'):
            self.open()
        self.assertEqual([name for name, _ in self.calls], ['challenge'])
        self.assertEqual(self.upgrades, 0)

    def test_upgrade_refusal_revokes_confirmed_candidate(self):
        self.fail = 'upgrade'
        with self.assertRaisesRegex(AssertionError, 'upgrade refused'):
            self.open()
        self.assertEqual(self.upgrades, 1)
        self.assertEqual(self.calls[-1], ('revoke', self.owner))

    def test_independent_renewal_preserves_session_then_failure_closes_socket(self):
        probe = self.open()
        try:
            initial = probe.owner.copy()
            deadline = time.monotonic() + 4
            while probe.owner == initial and time.monotonic() < deadline:
                time.sleep(0.01)
            self.assertGreater(probe.owner['issued_at_ms'], initial['issued_at_ms'])
            for key in ('session_id', 'epoch', 'sandbox_id', 'runtime_id', 'boot_id'):
                self.assertEqual(probe.owner[key], initial[key])
            self.assertEqual(self.calls[2], ('challenge', {
                'sandbox_id': 'fixture-sandbox', 'runtime_id': 'image-probe', 'previous': initial}))
            self.fail = 'challenge'
            probe.worker.join(timeout=4)
            self.assertFalse(probe.worker.is_alive())
            self.assertEqual(probe.socket.fileno(), -1)
        finally:
            with self.assertRaisesRegex(AssertionError, 'renewal failed'):
                probe.close()
        self.assertEqual(self.calls[-1], ('revoke', probe.owner or self.owner))

    def test_revoke_uncertainty_is_a_failure(self):
        probe = self.open()
        self.fail = 'revoke'
        with self.assertRaisesRegex(AssertionError, 'revoke refused'):
            probe.close()
        self.assertEqual(probe.socket.fileno(), -1)
        self.assertFalse(probe.worker.is_alive())

    def test_close_joins_pending_renewal_before_disconnecting(self):
        probe = self.open()
        entered, resume = threading.Event(), threading.Event()
        post = probe.post
        failures = []

        def delayed(action, body):
            if action == 'challenge':
                entered.set()
                assert resume.wait(2), 'test did not resume renewal'
            return post(action, body)

        def close():
            try:
                probe.close()
            except Exception as error:
                failures.append(error)

        probe.post = delayed
        closer = threading.Thread(target=close)
        try:
            self.assertTrue(entered.wait(3))
            closer.start()
            self.assertTrue(probe.stopped.wait(1))
            self.assertNotEqual(probe.socket.fileno(), -1)
        finally:
            resume.set()
            if closer.ident:
                closer.join(timeout=5)
            else:
                probe.close()
        self.assertFalse(closer.is_alive())
        self.assertEqual(failures, [])
        self.assertEqual(self.calls[-1], ('revoke', self.owner))

    def test_both_native_probes_use_protected_transport(self):
        for name in ('desktop-vnc-probe.py', 'omarchy-vnc-probe.py'):
            spec = importlib.util.spec_from_file_location('probe', Path(__file__).with_name(name))
            module = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(module)
            with patch.object(ProtectedDesktop, 'from_env', side_effect=self.open):
                stream = module.Stream()
            stream.transport.close()
        self.assertEqual(self.upgrades, 2)


if __name__ == '__main__':
    unittest.main()
