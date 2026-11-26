from concurrent.futures import ThreadPoolExecutor
import hashlib
import http.client
from http.server import ThreadingHTTPServer
import json
import os
from pathlib import Path
import secrets
import sys
import threading
import time
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent / 'cma'))
import provider


class TitleProviderTests(unittest.TestCase):
    def test_one_title_request_is_held_and_bound_to_its_exact_wire_body(self):
        control = secrets.token_urlsafe(32)
        marker = 'WS4_TITLE_' + 'f' * 32
        case_id = 'title-fixture'
        raw = json.dumps({'stream': False, 'store': False,
            'instructions': 'Create a 3-6 word conversation title. Return only the title, with no quotes or ornament.',
            'input': [{'role': 'user', 'content': marker}, {'role': 'assistant', 'content': 'Synthetic reply'}],
        }).encode()
        with patch.dict(os.environ, {'WS4_PROVIDER_CONTROL_TOKEN': control, 'WS4_PROVIDER_BEARER': ''}), \
             patch.object(provider, 'CASES', {}), patch.object(provider, 'ACTIVE', None), \
             ThreadingHTTPServer(('127.0.0.1', 0), provider.Handler) as server:
            worker = threading.Thread(target=server.serve_forever)
            worker.start()
            def post(route, body):
                client = http.client.HTTPConnection(*server.server_address, timeout=12)
                try:
                    client.request('POST', route, body, {'Authorization': 'Bearer ' + control,
                                                        'Content-Type': 'application/json'})
                    response = client.getresponse()
                    return response.status, json.loads(response.read())
                finally:
                    client.close()

            try:
                self.assertEqual(post('/v1/responses', raw)[0], 404)
                self.assertEqual(post('/__ws4/arm', json.dumps({'case_id': case_id,
                    'marker': marker, 'title': True}))[0], 200)
                with ThreadPoolExecutor(max_workers=1) as pool:
                    pending = pool.submit(post, '/v1/responses', raw)
                    try:
                        until = time.monotonic() + 3
                        while provider.snapshot(case_id)['title_requests'] == 0 and time.monotonic() < until:
                            time.sleep(.01)
                        view = provider.snapshot(case_id)
                        self.assertEqual(view['title_requests'], 1)
                        self.assertEqual(view['title_responses'], 0)
                        self.assertEqual(view['title_request_sha256'], hashlib.sha256(raw).hexdigest())
                        self.assertNotIn(marker, json.dumps(view))
                        self.assertFalse(pending.done())
                        self.assertEqual(post('/v1/responses', raw)[0], 409)
                    finally:
                        self.assertEqual(post('/__ws4/release-title', json.dumps({'case_id': case_id}))[0], 200)
                    self.assertEqual(pending.result(timeout=4), (200, {'output_text': 'Generated Fixture Conversation'}))
                self.assertEqual(provider.snapshot(case_id)['title_responses'], 1)
            finally:
                server.shutdown()
                worker.join(timeout=5)
                self.assertFalse(worker.is_alive())
