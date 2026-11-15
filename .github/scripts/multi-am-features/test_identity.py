"""Exercise the synthetic login redirect as an HTTP boundary."""
import contextlib
import http.client
from http.server import ThreadingHTTPServer
import io
from pathlib import Path
import sys
import threading
import unittest
from urllib.parse import parse_qs, urlencode, urlsplit

sys.path.insert(0, str(Path(__file__).resolve().parent / 'owner'))
from identity import Identity


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
