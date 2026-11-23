#!/usr/bin/env python3
"""Exercise the browser harness over HTTP; these are not image receipts."""
from contextlib import contextmanager
from http.cookies import SimpleCookie
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import secrets
import tempfile
import threading
import unittest
from urllib.parse import parse_qs, urlencode, urlsplit

from migration_browser_fixture import BrowserFixture, OWNER


class BrowserServer(ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self):
        super().__init__(('127.0.0.1', 0), BrowserHandler)
        self.attempts = set()
        self.sessions = set()
        self.revoked = set()
        self.request_count = 0
        self.mode = None


class BrowserHandler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def reply(self, status, body=None, headers=()):
        raw = json.dumps(body).encode() if body is not None else b''
        self.send_response(status)
        self.send_header('Content-Length', str(len(raw)))
        self.send_header('Content-Type', 'application/json')
        self.send_header('x-request-id', 'local-' + str(self.server.request_count))
        for name, value in headers:
            self.send_header(name, value)
        self.end_headers()
        self.wfile.write(raw)

    def do_GET(self):
        state = self.server
        state.request_count += 1
        path = urlsplit(self.path)
        jar = SimpleCookie(self.headers.get('Cookie', ''))
        session = jar['cma_session'].value if 'cma_session' in jar else None
        if path.path == '/auth/login':
            attempt = secrets.token_hex(24)
            state.attempts.add(attempt)
            return self.reply(303, headers=[
                ('Location', 'http://identity:8080/authorize?state=' + attempt),
                ('Set-Cookie', 'cma_login_' + attempt + '=' + secrets.token_hex(24)),
            ])
        if path.path == '/authorize':
            query = parse_qs(path.query, keep_blank_values=True)
            attempts = query.get('state', [])
            if set(query) != {'state'} or len(attempts) != 1 or attempts[0] not in state.attempts:
                return self.reply(400, {'code': 'login_state_invalid'})
            # Reflect only the bound attempt, encoded for this fixed callback.
            callback = urlencode({'state': attempts[0]})
            return self.reply(302, headers=[('Location',
                'https://api.localhost.direct/auth/callback?' + callback)])
        if path.path == '/auth/callback':
            attempt = parse_qs(path.query).get('state', [None])[0]
            if attempt not in state.attempts:
                return self.reply(400, {'code': 'login_state_invalid'})
            state.attempts.remove(attempt)
            session = secrets.token_hex(32)
            state.sessions.add(session)
            return self.reply(303, headers=[
                ('Location', '/'), ('Set-Cookie', 'cma_session=' + session),
            ])
        if path.path == '/auth/logout':
            if state.mode == 'logout_failure':
                return self.reply(503, {'code': 'store_unavailable'})
            state.revoked.add(session)
            headers = [('Location', '/wrong' if state.mode == 'logout_redirect' else '/')]
            if state.mode != 'logout_cookie':
                headers.append(('Set-Cookie', 'cma_session=; Max-Age=0'))
            return self.reply(303, headers=headers)
        if path.path == '/auth/session':
            revoked = session in state.revoked
            if revoked and state.mode != 'resurrect':
                if state.mode == 'wrong_denial':
                    return self.reply(403, {'authenticated': False})
                if state.mode == 'invalid_body':
                    return self.reply(401, {'authenticated': True})
                return self.reply(401, {'authenticated': False})
            if session in state.sessions:
                return self.reply(200, {'authenticated': True, 'subject': OWNER,
                                       'capabilities': ['sandbox:create'], 'visitor': False})
            return self.reply(401, {'authenticated': False})
        return self.reply(404, {'code': 'unsupported_fixture_route'})


@contextmanager
def campaign():
    with tempfile.TemporaryDirectory() as directory:
        server = BrowserServer()
        thread = threading.Thread(target=server.serve_forever,
                                  kwargs={'poll_interval': 0.01})
        thread.start()
        origin = 'http://127.0.0.1:' + str(server.server_port)
        state = Path(directory) / 'browser.private.json'
        try:
            yield server, lambda: BrowserFixture(origin, origin, state), state
        finally:
            server.shutdown()
            thread.join(timeout=5)
            server.server_close()
            if thread.is_alive():
                raise RuntimeError('browser fixture server failed to join')


class BrowserMigrationHTTPTests(unittest.TestCase):
    def test_deferred_callback_keeps_admission_sign_in_until_reads_finish(self):
        with campaign() as (server, factory, state):
            writer = factory()
            writer.start('old-write')
            writer.finish('old-write')
            saved = json.loads(state.read_text())
            retained = saved['session_cookies']
            pending = parse_qs(urlsplit(saved['pending']['authorization']).query)['state'][0]
            reader = factory()
            reader.defer_pending_callback = True
            self.assertEqual(reader.start('new-read-old-write'), retained)
            self.assertIn(pending, server.attempts)
            self.assertEqual(reader.check_session(), saved['session'])
            checks = reader.finish('new-read-old-write')
            self.assertTrue(checks['consumed_attempt_rejected'])
            self.assertNotIn(pending, server.attempts)
            self.assertNotEqual(reader.cookies, retained)
            self.assertEqual(json.loads(state.read_text()), saved)

    def test_deferred_callback_failure_cannot_be_reported_as_success(self):
        with campaign() as (server, factory, state):
            writer = factory()
            writer.start('new-write')
            writer.finish('new-write')
            saved = state.read_bytes()
            reader = factory()
            reader.defer_pending_callback = True
            reader.start('old-read-new-write')
            server.attempts.clear()
            with self.assertRaisesRegex(RuntimeError, 'provider_authorization_failed'):
                reader.finish('old-read-new-write')
            self.assertEqual(state.read_bytes(), saved)

    def test_authorize_rejects_unbound_ambiguous_or_injected_state(self):
        with campaign() as (server, factory, _):
            fixture = factory()
            attempt = fixture.begin()
            state = parse_qs(urlsplit(attempt['authorization']).query)['state'][0]
            queries = ['', 'state=', 'state=unknown', 'state=' + state + '&state=' + state,
                       'state=' + state + '&extra=value',
                       'state=' + state + '%0d%0aX-Injected%3A%20yes']
            for query in queries:
                with self.subTest(query_kind=queries.index(query)):
                    status, headers, body = fixture.raw('GET', fixture.identity + '/authorize?' + query)
                    self.assertEqual(status, 400)
                    self.assertEqual(json.loads(body), {'code': 'login_state_invalid'})
                    self.assertIsNone(headers.get('Location'))
                    self.assertIsNone(headers.get('X-Injected'))
                    self.assertIn(state, server.attempts)
            self.assertEqual(fixture.complete(attempt)['subject'], OWNER)

    def test_live_revoked_and_pending_records_survive_all_four_stages(self):
        with campaign() as (server, factory, state):
            revoked = []
            for stage in ('old-write', 'new-read-old-write', 'new-write', 'old-read-new-write'):
                fixture = factory()
                live = fixture.start(stage)
                checks = fixture.finish(stage)
                self.assertTrue(checks['revoked_session_rejected'])
                self.assertTrue(checks['session_owner_preserved'])
                self.assertEqual(checks['consumed_attempt_rejected'], 'read' in stage)
                self.assertIn(live['cma_session'], server.sessions)
                self.assertNotIn(live['cma_session'], server.revoked)
                revoked.append(fixture.revoked_cookies['cma_session'])
                saved = json.loads(state.read_text())
                self.assertIn(saved['revoked_session_cookies']['cma_session'], server.revoked)
                self.assertEqual(state.stat().st_mode & 0o777, 0o600)
                observations = json.dumps(fixture.observations)
                self.assertNotIn(live['cma_session'], observations)
                self.assertNotIn(fixture.revoked_cookies['cma_session'], observations)
                self.assertTrue(all(row['request_id'] for row in fixture.observations))
            self.assertEqual(revoked[0], revoked[1])
            self.assertEqual(revoked[2], revoked[3])
            self.assertNotEqual(revoked[0], revoked[2])

    def test_reader_rejects_resurrected_session_without_overwriting_writer_state(self):
        with campaign() as (server, factory, state):
            writer = factory()
            writer.start('new-write')
            writer.finish('new-write')
            before = state.read_bytes()
            server.mode = 'resurrect'
            with self.assertRaisesRegex(RuntimeError, 'browser_route_failed_/auth/session_200'):
                factory().start('old-read-new-write')
            self.assertEqual(state.read_bytes(), before)

    def test_denial_requires_401_and_explicit_anonymous_body(self):
        for mode, error in [('wrong_denial', 'session_403'),
                            ('invalid_body', 'not_anonymous')]:
            with self.subTest(mode=mode), campaign() as (server, factory, _):
                fixture = factory()
                fixture.start('old-write')
                server.mode = mode
                before = server.request_count
                with self.assertRaisesRegex(RuntimeError, error):
                    fixture.check_revoked_session()
                self.assertEqual(server.request_count, before + 1)

    def test_logout_failure_or_invalid_redirect_or_cookie_stops_writer(self):
        for mode, error in [('logout_failure', 'logout_503'),
                            ('logout_redirect', 'logout_contract_mismatch'),
                            ('logout_cookie', 'logout_contract_mismatch')]:
            with self.subTest(mode=mode), campaign() as (server, factory, state):
                server.mode = mode
                with self.assertRaisesRegex(RuntimeError, error):
                    factory().start('old-write')
                self.assertFalse(state.exists())
                self.assertEqual(len(server.sessions), 1)

    def test_retained_revocation_cannot_be_missing_or_alias_live_session(self):
        with campaign() as (server, factory, state):
            fixture = factory()
            fixture.start('old-write')
            fixture.finish('old-write')
            saved = json.loads(state.read_text())
            for revoked in (None, {}, [], saved['session_cookies']):
                with self.subTest(kind=type(revoked).__name__):
                    saved['revoked_session_cookies'] = revoked
                    state.write_text(json.dumps(saved))
                    before = server.request_count
                    with self.assertRaisesRegex(RuntimeError, 'missing_or_live'):
                        factory().start('new-read-old-write')
                    self.assertEqual(server.request_count, before)

    def test_writer_provenance_cannot_be_relabelled(self):
        with campaign() as (server, factory, _):
            fixture = factory()
            fixture.start('old-write')
            fixture.finish('old-write')
            before = server.request_count
            with self.assertRaisesRegex(RuntimeError, 'writer_identity_mismatch'):
                factory().start('old-read-new-write')
            self.assertEqual(server.request_count, before)

    def test_writer_restricts_existing_state_file_permissions(self):
        with campaign() as (_, factory, state):
            state.write_text('{}')
            state.chmod(0o644)
            fixture = factory()
            fixture.start('old-write')
            fixture.finish('old-write')
            self.assertEqual(state.stat().st_mode & 0o777, 0o600)


if __name__ == '__main__':
    unittest.main()
