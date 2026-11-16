import hashlib
import http.client
from http.server import ThreadingHTTPServer
import json
from pathlib import Path
import secrets
import sys
import threading
import unittest
from urllib.parse import urlencode

sys.path.insert(0, str(Path(__file__).resolve().parent / 'owner'))
from storage import PREFIX, Storage


class StorageBoundaryTests(unittest.TestCase):
    def setUp(self):
        class Handler(Storage):
            config = {'client_id': 'fixture', 'client_secret': secrets.token_hex(32), 'access': secrets.token_hex(32)}
            objects = {}
        self.handler = Handler
        self.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.addCleanup(self.close)

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=5)
        self.assertFalse(self.thread.is_alive())

    def request(self, method, path, body=None, authenticated=True):
        connection = http.client.HTTPConnection(*self.server.server_address, timeout=5)
        try:
            headers = {'Authorization': 'Bearer ' + self.handler.config['access']} if authenticated else {}
            connection.request(method, path, body=body, headers=headers)
            response = connection.getresponse()
            return response.status, response.read()
        finally:
            connection.close()

    def object(self, body):
        digest = hashlib.sha256(body).hexdigest()
        key = 'documents/' + 'a' * 64 + '/doc_' + 'b' * 32 + '/' + digest + '.md'
        return key, {'key': key, 'contentType': 'text/markdown; charset=utf-8',
                     'mutationGeneration': '1', 'contentSha256': digest}

    def test_machine_token_requires_exact_client_and_scope(self):
        form = {'grant_type': 'client_credentials', 'client_id': 'fixture',
                'client_secret': self.handler.config['client_secret'],
                'scope': 'proxy object:create object:read object:download'}
        status, raw = self.request('POST', '/oauth/token', urlencode(form), authenticated=False)
        self.assertEqual(status, 200)
        self.assertEqual(json.loads(raw)['access_token'], self.handler.config['access'])
        form['client_secret'] = 'wrong'
        self.assertEqual(self.request('POST', '/oauth/token', urlencode(form))[0], 401)

    def test_object_receipt_readback_and_idempotent_content_fence(self):
        body = b'# actual document bytes\n'
        key, query = self.object(body)
        path = PREFIX + '?' + urlencode(query)
        for _ in range(2):
            status, raw = self.request('POST', path, body)
            self.assertEqual(status, 200)
            self.assertEqual(json.loads(raw)['data'], {'key': key, 'contentLength': len(body),
                'generation': 1, 'digest': query['contentSha256'], 'fenceVersion': 1})
        self.assertEqual(self.request('POST', path, b'changed')[0], 409)
        query['mutationGeneration'] = '2'
        self.assertEqual(self.request('POST', PREFIX + '?' + urlencode(query), body)[0], 409)
        self.assertEqual(self.request('GET', PREFIX + '/download?' + urlencode({'key': key})), (200, body))
        self.assertEqual(len(self.handler.objects), 1)

    def test_foreign_route_key_and_missing_authority_are_refused(self):
        body = b'fixture'
        key, query = self.object(body)
        self.assertEqual(self.request('POST', PREFIX + '?' + urlencode(query), body, authenticated=False)[0], 401)
        self.assertEqual(self.request('POST', PREFIX.replace('cma-ci-documents', 'another-bucket') + '?' + urlencode(query), body)[0], 404)
        self.assertEqual(self.request('GET', PREFIX + '/download?' + urlencode({'key': '../foreign'}))[0], 400)
        self.assertEqual(self.request('GET', PREFIX + '/download?' + urlencode({'key': key}))[0], 404)


if __name__ == '__main__':
    unittest.main()
