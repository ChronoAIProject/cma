"""Real loopback connect-link protocol, custody, bounds and manifest checks."""
import base64
from contextlib import redirect_stdout
from copy import deepcopy
from http.server import ThreadingHTTPServer
import importlib.util
import io
import json
from pathlib import Path
import secrets
import threading
import unittest
from urllib.error import HTTPError
from urllib.request import Request, urlopen

from migration_kind_manifests import manifests as base_manifests
from migration_launch_provider import SERVICE, STATES, handler, manifests
from test_migration_kind_runner import IMAGE, MONGO


class ProviderTests(unittest.TestCase):
    def setUp(self):
        source = Path(__file__).with_name('multi-am-features') / 'owner/identity.py'
        spec = importlib.util.spec_from_file_location('launch_identity', source)
        self.identity = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.identity)
        self.provider = handler(self.identity)
        self.credentials = {key: secrets.token_urlsafe(32) for key in (
            'bearer', 'admin_bearer', 'outsider_bearer', 'client_id', 'client_secret', 'refresh_token')}
        self.credentials['expires_at'] = 2000000000
        self.provider.config = self.credentials
        self.server = ThreadingHTTPServer(('127.0.0.1', 0), self.provider)
        self.server.daemon_threads = True
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.output = io.StringIO()
        self.redirect = redirect_stdout(self.output)
        self.redirect.__enter__()
        self.thread.start()
        self.addCleanup(self.finish)

    def finish(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=2)
        self.redirect.__exit__(None, None, None)
        self.assertFalse(self.thread.is_alive())
        for value in self.credentials.values():
            self.assertNotIn(str(value), self.output.getvalue())

    def request(self, method, route, body=None, actor='bearer', status=200):
        headers = {} if actor is None else {'Authorization': 'Bearer ' + self.credentials[actor]}
        data = json.dumps(body).encode() if body is not None else None
        req = Request('http://127.0.0.1:' + str(self.server.server_port) + route,
                      method=method, headers=headers, data=data)
        try:
            with urlopen(req, timeout=3) as result:
                code, raw = result.status, result.read()
        except HTTPError as error:
            code, raw = error.code, error.read()
        self.assertEqual(code, status)
        return json.loads(raw)

    def create(self):
        return self.request('POST', '/api/v1/connect-links', {
            'service_slug': SERVICE, 'expires_in': 900,
            'callback_url': 'https://api.localhost.direct/connection-return'}, status=201)

    def test_real_link_transitions_and_default_credentials(self):
        created = self.create()
        self.assertRegex(created['connect_url'], r'^https://identity.local.test/connect/nyx_clk_[a-f0-9]{64}$')
        key = created['id']
        for state in STATES:
            changed = self.request('POST', '/fixture/launch/' + key + '/' + state, actor='admin_bearer')
            self.assertEqual(changed['links'][key], state)
            row = self.request('GET', '/api/v1/connect-links/' + key, status=404 if state == 'refused' else 200)
            services = self.request('GET', '/api/v1/user-services')['services']
            self.assertEqual(len(services), len(self.identity.SERVICES) + (state == 'completed'))
            if state != 'refused':
                self.assertEqual(row['id'], key)
                self.assertEqual(row['status'], state)
                self.assertEqual(row['service_slug'], SERVICE)
            if state == 'completed':
                self.assertEqual(row['connected_service']['slug'], SERVICE)
        observed = self.request('GET', '/fixture/launch/state', actor='admin_bearer')
        self.assertEqual((observed['creates'], observed['reads']), (1, 5))
        self.assertEqual(self.request('GET', '/api/v1/services')['services'][0]['slug'], SERVICE)
        self.assertTrue(self.request('GET', '/api/v1/keys/fixture-matrix-connect')['connected'])
        self.assertTrue(self.request('GET', '/fixture/health')['ok'])

    def test_forbidden_control_does_not_change_links(self):
        key = self.create()['id']
        for actor in ('bearer', 'outsider_bearer', None):
            self.request('GET', '/fixture/launch/state', actor=actor, status=403)
            self.request('POST', '/fixture/launch/' + key + '/completed', actor=actor, status=403)
        self.assertEqual(self.provider.links[key]['status'], 'pending')
        for route in ('/api/v1/user-services', '/api/v1/services', '/api/v1/connect-links/' + key):
            self.request('GET', route, actor=None, status=401)
        self.request('POST', '/api/v1/connect-links', {}, actor=None, status=401)

    def test_invalid_input_and_unknown_link_are_rejected(self):
        for body in ({}, [], {'service_slug': 'other'}):
            self.request('POST', '/api/v1/connect-links', body, status=400)
        for suffix in ('bogus', 'link_' + 'a'*32 + '/unknown'):
            self.request('POST', '/fixture/launch/' + suffix, actor='admin_bearer', status=400)
        self.request('POST', '/fixture/launch/link_' + 'a'*32 + '/pending', actor='admin_bearer', status=404)
        key = self.create()['id']
        self.request('POST', '/fixture/launch/' + key + '/pending', {}, actor='admin_bearer', status=400)
        self.request('GET', '/api/v1/connect-links/unknown', status=404)
        self.request('POST', '/unsupported', {}, status=404)

    def test_link_capacity_is_finite(self):
        for _ in range(10):
            self.create()
        self.request('POST', '/api/v1/connect-links', {'service_slug': SERVICE, 'expires_in': 900,
            'callback_url': 'https://api.localhost.direct/connection-return'}, status=429)
        self.assertEqual(self.provider.creates, 10)

    def test_manifest_denies_native_mutations_and_keeps_exact_images(self):
        original = base_manifests(IMAGE, secrets.token_urlsafe(32), secrets.token_urlsafe(32),
            base64.b64encode(secrets.token_bytes(32)).decode(), MONGO,
            {'createRole': 'cma_am_runtime', 'roles': [], 'privileges': []}, identity_credentials=self.credentials)
        before = deepcopy(original)
        changed = manifests(original)
        self.assertEqual(original, before)
        for obj in changed:
            self.assertNotEqual(obj['kind'], 'Secret')
            if obj['kind'] == 'Role':
                self.assertTrue(all(rule['verbs'] == ['get', 'list'] for rule in obj['rules']))
            if obj['kind'] == 'Deployment':
                old = next(x for x in before if x['kind'] == 'Deployment' and x['metadata'] == obj['metadata'])
                self.assertEqual(obj['spec']['template']['spec']['containers'][0]['image'],
                                 old['spec']['template']['spec']['containers'][0]['image'])
        with self.assertRaisesRegex(ValueError, 'already customized'):
            manifests(changed)


if __name__ == '__main__':
    unittest.main()
