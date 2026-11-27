from email.message import Message
from http.client import RemoteDisconnected
from pathlib import Path
import secrets
import sys
import tempfile
import unittest
from unittest.mock import Mock

sys.path.insert(0, str(Path(__file__).resolve().parent / 'owner'))
from login import login


def redirect(location, *cookies):
    headers = Message()
    headers['Location'] = location
    for cookie in cookies:
        headers.add_header('Set-Cookie', cookie)
    return 302, headers, b''


class BrowserLoginTests(unittest.TestCase):
    def exercise(self, cookie, attributes, directory):
        client = Mock(output=Path(directory))
        client.forward.return_value = 'http://127.0.0.1:30001'
        client.http_raw.side_effect = [
            redirect('http://identity:8080/authorize?state=fixture', 'cma_login_fixture=binding'),
            redirect('https://api.localhost.direct/auth/callback?code=fixture'),
            redirect('/', 'cma_session=' + cookie + attributes),
        ]
        stages = []
        observed = login(client, 'http://127.0.0.1:30002', public_host='api.localhost.direct',
                         mark_stage=stages.append)
        self.assertEqual(stages, ['am_login', 'identity_forward', 'identity_authorize', 'am_callback'])
        self.assertEqual(client.http_raw.call_count, 3)
        callback = client.http_raw.call_args
        self.assertEqual(callback.args[2], '/auth/callback?code=fixture')
        self.assertEqual(callback.kwargs['headers']['Cookie'], 'cma_login_fixture=binding')
        return observed

    def test_transport_failure_keeps_the_last_attempted_hop(self):
        client = Mock()
        client.http_raw.side_effect = [
            redirect('http://identity:8080/authorize?state=fixture', 'cma_login_fixture=binding'),
            redirect('https://api.localhost.direct/auth/callback?code=fixture'),
            RemoteDisconnected(),
        ]
        stages = []
        with self.assertRaises(RemoteDisconnected):
            login(client, 'http://127.0.0.1:30002', public_host='api.localhost.direct',
                  mark_stage=stages.append)
        self.assertEqual(stages[-1], 'am_callback')

    def test_returns_live_cookie_without_writing_it_to_evidence(self):
        cookie = secrets.token_urlsafe(32)
        with tempfile.TemporaryDirectory() as directory:
            credentials = self.exercise(cookie, '; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=60', directory)
            self.assertEqual(credentials, cookie)
            evidence = (Path(directory) / 'owner-login.json').read_text()
            self.assertNotIn(cookie, evidence)
            self.assertNotIn('code=fixture', evidence)

    def test_refuses_non_secure_or_expired_cookie(self):
        with tempfile.TemporaryDirectory() as directory:
            for attributes in ['; Path=/; HttpOnly; SameSite=Lax; Max-Age=60',
                               '; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=0']:
                with self.subTest(attributes=attributes), self.assertRaisesRegex(RuntimeError, 'real_browser_session_cookie_missing'):
                    self.exercise(secrets.token_urlsafe(32), attributes, directory)
            self.assertEqual(list(Path(directory).iterdir()), [])
