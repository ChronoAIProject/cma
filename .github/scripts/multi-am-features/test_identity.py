"""Exercise the synthetic login redirect as an HTTP boundary."""
import contextlib
import http.client
from http.server import ThreadingHTTPServer
import io
from pathlib import Path
import secrets
import sys
import threading
import time
import unittest
from urllib.parse import parse_qs, urlencode, urlsplit

sys.path.insert(0, str(Path(__file__).resolve().parent / 'owner'))
from identity import Identity


class IndependentActorTests(unittest.TestCase):
    def setUp(self):
        self.credentials = {key: secrets.token_urlsafe(32) for key in (
            'bearer', 'admin_bearer', 'outsider_bearer', 'refresh_token', 'client_id', 'client_secret')}
        self.credentials['expires_at'] = int(time.time()) + 3600
        class Handler(Identity):
            config = self.credentials
        self.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.addCleanup(self.close)

    def close(self):
        self.server.shutdown(); self.server.server_close(); self.thread.join(timeout=5)
        self.assertFalse(self.thread.is_alive())

    def request(self, method, path, *, token=None, form=None):
        import json
        client = http.client.HTTPConnection(*self.server.server_address, timeout=5)
        try:
            with contextlib.redirect_stdout(io.StringIO()):
                client.request(method, path, body=urlencode(form) if form else None,
                               headers={'Authorization': 'Bearer ' + token} if token else {})
                response = client.getresponse(); body = json.loads(response.read())
                return response.status, body
        finally:
            client.close()

    def test_actors_verify_independently_and_outsider_gets_no_admin_authority(self):
        subjects = []
        for key in ['bearer', 'admin_bearer', 'outsider_bearer']:
            token = self.credentials[key]
            status, user = self.request('GET', '/userinfo', token=token)
            self.assertEqual(status, 200); subjects.append(user['sub'])
            status, observed = self.request('POST', '/introspect', form={
                'token': token, 'token_type_hint': 'access_token',
                'client_id': self.credentials['client_id'], 'client_secret': self.credentials['client_secret']})
            self.assertEqual(status, 200); self.assertEqual(observed['sub'], user['sub'])
            self.assertEqual(user['permissions'], observed['permissions'])
            self.assertEqual(any(x.startswith('cma:admin:') for x in observed['permissions']), key == 'admin_bearer')
        self.assertEqual(len(set(subjects)), 3)
        status, _ = self.request('GET', '/api/v1/services', token=self.credentials['outsider_bearer'])
        self.assertEqual(status, 401)

    def test_unknown_token_does_not_fall_back_to_owner(self):
        self.assertEqual(self.request('GET', '/userinfo', token='unknown-fixture-input')[0], 401)
        status, body = self.request('POST', '/introspect', form={
            'token': 'unknown-fixture-input', 'token_type_hint': 'access_token',
            'client_id': self.credentials['client_id'], 'client_secret': self.credentials['client_secret']})
        self.assertEqual(status, 401); self.assertFalse(body['active'])


class LoginRedirectTests(unittest.TestCase):
    def setUp(self):
        class Handler(Identity):
            config = {'client_id': 'test-client'}
            codes = {}
        self.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.addCleanup(self.close)

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=5)
        self.assertFalse(self.thread.is_alive())

    def authorize(self, **changes):
        params = {'client_id': 'test-client', 'redirect_uri': 'https://api.localhost.direct/auth/callback',
                  'response_type': 'code', 'code_challenge_method': 'S256',
                  'state': 'fixture-state', 'code_challenge': 'x' * 43, **changes}
        connection = http.client.HTTPConnection(*self.server.server_address, timeout=5)
        try:
            with contextlib.redirect_stdout(io.StringIO()):
                connection.request('GET', '/authorize?' + urlencode(params))
                response = connection.getresponse()
                body = response.read()
            return response.status, dict(response.getheaders()), body
        finally:
            connection.close()

    def test_state_is_encoded_and_callback_is_fixed(self):
        state = 'fixture\r\nLocation: https://unrelated.invalid'
        status, headers, _ = self.authorize(state=state)
        self.assertEqual(status, 302)
        callback = urlsplit(headers['Location'])
        self.assertEqual((callback.scheme, callback.netloc, callback.path),
                         ('https', 'api.localhost.direct', '/auth/callback'))
        self.assertEqual(parse_qs(callback.query)['state'], [state])
        self.assertNotIn('\r', headers['Location'])
        self.assertNotIn('\n', headers['Location'])

    def test_foreign_or_injected_destination_is_refused(self):
        for destination in ['https://unrelated.invalid/callback',
                            'https://api.localhost.direct/auth/callback\r\nX-Test: injected']:
            status, headers, _ = self.authorize(redirect_uri=destination)
            self.assertEqual(status, 400)
            self.assertNotIn('Location', headers)


if __name__ == '__main__':
    unittest.main()
