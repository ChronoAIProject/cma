import contextlib
import http.client
from http.server import ThreadingHTTPServer
import json
import os
from pathlib import Path
import secrets
import sys
import threading
import unittest
from unittest.mock import patch
from uuid import uuid4

sys.path.insert(0, str(Path(__file__).resolve().parent / 'cma'))
import provider


class AccessProviderTests(unittest.TestCase):
    def test_both_declared_tool_formats_use_the_same_bounded_proposal(self):
        name = 'mcp__cma_access__agent_key_scope_revision'
        request_key = str(uuid4())
        direct = {'tools': [{'name': name}]}
        code = {'input': [{'type': 'additional_tools', 'tools': [
            {'name': 'functions', 'tools': [{'name': 'exec'}]},
        ]}]}
        item = provider.access_tool_events('response', direct, request_key, 'call')[-1]['response']['output'][0]
        self.assertEqual(item['call_id'], 'call')
        self.assertEqual(item['type'], 'function_call')
        proposal = json.loads(item['arguments'])
        self.assertEqual(proposal['services'], ['fixture-access-service'])
        self.assertEqual(proposal['request_key'], request_key)
        item = provider.access_tool_events('response', code, request_key, 'call')[-1]['response']['output'][0]
        self.assertEqual(item['type'], 'custom_tool_call')
        self.assertIn(json.dumps(name), item['input'])
        self.assertIn(json.dumps(proposal), item['input'])
        with self.assertRaisesRegex(ValueError, 'fixture_access_tool_undeclared'):
            provider.access_tool_events('response', {'tools': [{'name': 'unrelated'}]}, request_key, 'call')

    def test_single_tool_result_finishes_one_armed_turn(self):
        control = secrets.token_urlsafe(32)
        with patch.dict(os.environ, {'WS4_PROVIDER_CONTROL_TOKEN': control, 'WS4_PROVIDER_BEARER': ''}), \
             patch.object(provider, 'CASES', {}), patch.object(provider, 'ACTIVE', None):
            server = ThreadingHTTPServer(('127.0.0.1', 0), provider.Handler)
            thread = threading.Thread(target=server.serve_forever)
            thread.start()
            try:
                def request(route, body):
                    connection = http.client.HTTPConnection(*server.server_address, timeout=5)
                    with contextlib.closing(connection):
                        connection.request('POST', route, json.dumps(body), {
                            'Content-Type': 'application/json', 'Authorization': 'Bearer ' + control,
                        })
                        response = connection.getresponse()
                        return response.status, response.read().decode()

                fixture = {'case_id': 'access-case', 'marker': 'ACCESS_MARKER', 'access_request': str(uuid4())}
                self.assertEqual(request('/__ws4/arm', fixture)[0], 200)
                self.assertEqual(request('/__ws4/release', {'case_id': fixture['case_id']})[0], 200)
                body = {'stream': True, 'tools': [{'name': 'mcp__cma_access__agent_key_scope_revision'}],
                        'input': [{'type': 'message', 'content': 'ACCESS_MARKER'}]}
                status, data = request('/v1/responses', body)
                self.assertEqual(status, 200)
                tail = json.loads([line[6:] for line in data.splitlines() if line.startswith('data: ')][-1])
                call_id = tail['response']['output'][0]['call_id']
                self.assertEqual(request('/v1/responses', body)[0], 409)
                request_id = 'aar_' + 'a' * 64
                status, data = request('/v1/responses', {'stream': True, 'input': [
                    {'type': 'function_call_output', 'call_id': call_id, 'output': json.dumps({
                        'request_id': request_id, 'status': 'pending', 'private': 'synthetic result',
                    })},
                ]})
                self.assertEqual(status, 200)
                self.assertIn('WS4 complete ACCESS_MARKER', data)
                observed = provider.snapshot('access-case')
                self.assertEqual(observed['request_count'], 2)
                self.assertEqual(observed['access_tool_calls'], 1)
                self.assertEqual(observed['completed_effect_count'], 2)
                self.assertEqual(observed['access_receipt'], {'request_sha256': provider.digest(request_id),
                                                             'status': 'pending'})
            finally:
                server.shutdown()
                server.server_close()
                thread.join(timeout=5)
                self.assertFalse(thread.is_alive())
