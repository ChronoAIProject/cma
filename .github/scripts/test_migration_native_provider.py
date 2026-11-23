"""Real loopback HTTP/SSE observations for the reused synthetic provider."""
import http.client
import io
import json
import os
import secrets
import ssl
import threading
import unittest
from http.server import ThreadingHTTPServer
from unittest.mock import Mock, patch

import migration_native_provider as provider


class NativeProviderTests(unittest.TestCase):
    def setUp(self):
        self.token = secrets.token_urlsafe(32)
        patches = [patch.dict(os.environ, {'WS4_PROVIDER_CONTROL_TOKEN': self.token,
                                          'WS4_PROVIDER_BEARER': '', 'WS4_PROVIDER_ALLOW_BOOTSTRAP': 'false'}),
                   patch.object(provider, 'CASES', {}), patch.object(provider, 'ACTIVE', None),
                   patch.object(provider, 'BOOTSTRAP', {
                       'request_count': 0, 'completed_effect_count': 0, 'disconnect_count': 0,
                       'active_streams': 0, 'request_hashes': [], 'response_hashes': [], 'events': [],
                       'marker': 'WS4_READY', 'release': threading.Event()})]
        for item in patches:
            item.start()
            self.addCleanup(item.stop)
        provider.BOOTSTRAP['release'].set()
        self.server = ThreadingHTTPServer(('127.0.0.1', 0), provider.Handler)
        self.worker = threading.Thread(target=self.server.serve_forever, kwargs={'poll_interval': 0.01})
        self.worker.start()
        self.addCleanup(self.close)

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.worker.join(2)
        self.assertFalse(self.worker.is_alive())

    def request(self, method, route, body=None, *, token=None, raw=None):
        connection = http.client.HTTPConnection('127.0.0.1', self.server.server_port, timeout=4)
        try:
            headers = {} if token is None else {'Authorization': 'Bearer ' + token}
            payload = raw if raw is not None else (json.dumps(body) if body is not None else None)
            connection.request(method, route, payload, headers)
            response = connection.getresponse()
            data = response.read().decode()
            return response.status, data, dict(response.getheaders())
        finally:
            connection.close()

    def test_controls_release_one_actual_stream_and_return_only_hashes(self):
        case, marker = 'migration-stage', secrets.token_hex(16)
        self.assertEqual(self.request('POST', '/__ws4/arm', {'case_id': case, 'marker': marker}, token=self.token)[0], 200)
        self.assertEqual(self.request('POST', '/__ws4/release', {'case_id': case}, token=self.token)[0], 200)
        status, data, headers = self.request('POST', '/v1/responses', {
            'stream': True, 'model': 'fixture', 'input': marker})
        self.assertEqual(status, 200)
        self.assertEqual(headers['Content-Type'], 'text/event-stream')
        events = [json.loads(line[6:]) for line in data.splitlines() if line.startswith('data: ')]
        self.assertEqual([e['sequence_number'] for e in events], list(range(9)))
        self.assertEqual(events[-1]['type'], 'response.completed')
        self.assertEqual(events[-1]['response']['output'][0]['content'][0]['text'], 'WS4 complete ' + marker)
        status, body, _ = self.request('GET', '/__ws4/state?case_id=' + case, token=self.token)
        state = json.loads(body)
        self.assertEqual(status, 200)
        self.assertEqual((state['request_count'], state['completed_effect_count'], state['active_streams']), (1, 1, 0))
        self.assertNotIn(marker, body)
        self.assertNotIn(self.token, body)

    def test_denials_invalid_bodies_and_unknown_cases(self):
        for method, path, body, status in [
            ('GET', '/health', None, 200), ('GET', '/missing', None, 404),
            ('GET', '/__ws4/state?case_id=missing', None, 401),
            ('POST', '/__ws4/arm', {'case_id': 'x', 'marker': 'marker-long'}, 401),
            ('POST', '/responses', {'stream': False}, 400),
            ('POST', '/responses', {'stream': True, 'input': 'missing'}, 409),
            ('POST', '/missing', {}, 404),
        ]:
            with self.subTest(path=path, method=method):
                self.assertEqual(self.request(method, path, body)[0], status)
        for path in ('/__ws4/state?case_id=missing', '/__ws4/state'):
            self.assertEqual(self.request('GET', path, token=self.token)[0], 404)
        self.assertEqual(self.request('POST', '/__ws4/missing', {'case_id': 'x'}, token=self.token)[0], 404)
        self.assertEqual(self.request('POST', '/__ws4/release', {}, token=self.token)[0], 409)
        self.assertEqual(self.request('POST', '/responses', raw='invalid-json')[0], 400)
        self.assertEqual(self.request('POST', '/responses', raw='')[0], 400)
        for raw in ('null', '[]', '3', '"value"'):
            self.assertEqual(self.request('POST', '/responses', raw=raw)[0], 400)
        self.assertEqual(self.request('POST', '/__ws4/release', {'case_id': []}, token=self.token)[0], 409)
        for case, marker in [('', 'validmarker'), ('x', 'short'), (4, 'validmarker'), ('x', None)]:
            self.assertEqual(self.request('POST', '/__ws4/arm', {'case_id': case, 'marker': marker}, token=self.token)[0], 409)
        provider.arm('unique', 'marker-long')
        with self.assertRaisesRegex(ValueError, 'already_exists'):
            provider.arm('unique', 'marker-long')

    def test_model_bearer_and_bootstrap_are_explicit(self):
        model_token = secrets.token_urlsafe(32)
        with patch.dict(os.environ, {'WS4_PROVIDER_BEARER': model_token, 'WS4_PROVIDER_ALLOW_BOOTSTRAP': 'true'}):
            self.assertEqual(self.request('GET', '/health')[0], 401)
            self.assertEqual(self.request('GET', '/health', token=self.token)[0], 200)
            self.assertEqual(self.request('GET', '/health', token=model_token)[0], 200)
            self.assertEqual(self.request('POST', '/responses', {'stream': True})[0], 401)
            status, data, _ = self.request('POST', '/responses', {'stream': True}, token=model_token)
            self.assertEqual(status, 200)
            self.assertIn('response.completed', data)
            self.assertEqual(self.request('GET', '/__ws4/bootstrap')[0], 401)
            state = json.loads(self.request('GET', '/__ws4/bootstrap', token=self.token)[1])
            self.assertEqual(state['completed_effect_count'], 1)

    def test_peer_close_checks_distinguish_tls_would_block_from_eof(self):
        handler = object.__new__(provider.Handler)
        handler.connection = Mock()
        handler.connection.gettimeout.return_value = 7
        with patch.object(provider.select, 'select', return_value=([], [], [])):
            self.assertFalse(handler.peer_closed())
        for result in (b'', b'x', ssl.SSLWantReadError(), ssl.SSLWantWriteError(), BlockingIOError()):
            handler.connection.recv.side_effect = result if isinstance(result, Exception) else None
            handler.connection.recv.return_value = result
            with patch.object(provider.select, 'select', return_value=([handler.connection], [], [])):
                self.assertEqual(handler.peer_closed(), result == b'')
            handler.connection.settimeout.assert_called_with(7)

    def test_held_disconnect_and_deadline_release_active_count_without_completion(self):
        for mode in ('disconnect', 'deadline', 'release-after-heartbeat'):
            provider.arm(mode, 'marker-for-' + mode)
            case = provider.CASES[mode]
            handler = object.__new__(provider.Handler)
            handler.path = '/responses'
            handler.body = Mock(return_value={'stream': True, 'input': 'marker-for-' + mode})
            handler.authorized_model = Mock(return_value=True)
            handler.send_response = Mock()
            handler.send_header = Mock()
            handler.end_headers = Mock()
            handler.peer_closed = Mock(return_value=mode == 'disconnect')
            handler.wfile = io.BytesIO()
            case['release'] = Mock()
            case['release'].wait.side_effect = [False, True]
            clock = [0, 601] if mode == 'deadline' else [0, 1]
            with patch.object(provider.time, 'monotonic', side_effect=clock):
                handler.do_POST()
            self.assertEqual(case['active_streams'], 0)
            if mode == 'release-after-heartbeat':
                self.assertIn(b': ws4-held', handler.wfile.getvalue())
                self.assertEqual(case['completed_effect_count'], 1)
            else:
                self.assertEqual(case['completed_effect_count'], 0)
                self.assertEqual(case['disconnect_count'], 1)
            self.assertTrue(handler.close_connection)


if __name__ == '__main__':
    unittest.main()
