from concurrent.futures import ThreadPoolExecutor
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


class SuggestionProviderTests(unittest.TestCase):
    def test_only_marked_answer_waits_for_the_separate_gate(self):
        control = secrets.token_urlsafe(32)
        marker = 'WS4_SUGGEST_' + 'b' * 32
        case = {'case_id': 'suggestion-case', 'marker': marker, 'suggestions': True}
        with patch.dict(os.environ, {'WS4_PROVIDER_CONTROL_TOKEN': control, 'WS4_PROVIDER_BEARER': ''}), \
             patch.object(provider, 'CASES', {}), patch.object(provider, 'ACTIVE', None):
            with ThreadingHTTPServer(('127.0.0.1', 0), provider.Handler) as server:
                thread = threading.Thread(target=server.serve_forever)
                thread.start()

                def request(path, data):
                    connection = http.client.HTTPConnection(*server.server_address, timeout=5)
                    try:
                        connection.request('POST', path, json.dumps(data), {
                            'Content-Type': 'application/json', 'Authorization': 'Bearer ' + control,
                        })
                        response = connection.getresponse()
                        return response.status, json.loads(response.read())
                    finally:
                        connection.close()

                def suggestion(answer):
                    return request('/v1/responses', {
                        'stream': False, 'text': {'format': {'name': 'contextual_suggestions'}},
                        'input': [{'role': 'user', 'content': json.dumps({'answer': answer})}],
                    })

                def items(result):
                    status, body = result
                    self.assertEqual(status, 200)
                    return json.loads(body['output'][0]['content'][0]['text'])['suggestions']

                try:
                    self.assertEqual(suggestion(marker)[0], 409)
                    self.assertEqual(items(suggestion('unrelated answer')), [])
                    self.assertEqual(request('/__ws4/arm', case)[0], 200)
                    self.assertEqual(request('/__ws4/release', {'case_id': case['case_id']})[0], 200)
                    with ThreadPoolExecutor(max_workers=1) as runner:
                        pending = runner.submit(suggestion, 'WS4 complete ' + marker)
                        try:
                            deadline = time.monotonic() + 3
                            while not provider.snapshot(case['case_id'])['suggestion_requests'] and time.monotonic() < deadline:
                                time.sleep(.01)
                            view = provider.snapshot(case['case_id'])
                            self.assertEqual(view['suggestion_requests'], 1)
                            self.assertTrue(view['released'])
                            self.assertFalse(view['suggestion_released'])
                            self.assertFalse(pending.done())
                            self.assertEqual(suggestion(marker)[0], 409)
                        finally:
                            self.assertEqual(request('/__ws4/release-suggestion', {'case_id': case['case_id']})[0], 200)
                        self.assertEqual(items(pending.result(timeout=5)), [{
                            'text': 'Explain the fixture result in one sentence.', 'evidence': marker,
                        }])
                    view = provider.snapshot(case['case_id'])
                    self.assertEqual(view['suggestion_responses'], 1)
                    self.assertEqual(view['request_count'], 0)
                finally:
                    server.shutdown()
                    thread.join(timeout=5)
                    self.assertFalse(thread.is_alive())
