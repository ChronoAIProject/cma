"""Real HTTP checks: credential custody is bounded and independently verified."""
from contextlib import redirect_stdout
from copy import deepcopy
from http.server import ThreadingHTTPServer
import importlib.util
import io
import json
import os
from pathlib import Path
import secrets
import threading
import time
from types import SimpleNamespace
import unittest
from unittest.mock import patch
from urllib.error import HTTPError
from urllib.parse import urlencode
from urllib.request import Request, urlopen

from migration_api_custody_provider import SLOTS, handler, manifests, metadata
from migration_api_custody_fixture import fetch
from migration_browser_fixture import OWNER
from migration_kind_manifests import manifests as base_manifests


class ProviderTests(unittest.TestCase):
    def setUp(self):
        path = Path(__file__).with_name('multi-am-features') / 'owner' / 'identity.py'
        spec = importlib.util.spec_from_file_location('custody_test_identity', path)
        identity = importlib.util.module_from_spec(spec); spec.loader.exec_module(identity)
        self.provider = handler(identity)
        self.credentials = {k: secrets.token_urlsafe(32) for k in (
            'bearer', 'admin_bearer', 'outsider_bearer', 'client_id', 'client_secret', 'refresh_token')}
        self.credentials['expires_at'] = int(time.time()) + 3600
        self.provider.config = self.credentials
        self.server = ThreadingHTTPServer(('127.0.0.1', 0), self.provider)
        self.server.daemon_threads = True
        self.base = 'http://127.0.0.1:' + str(self.server.server_port)
        self.log = io.StringIO(); self.redirect = redirect_stdout(self.log); self.redirect.__enter__()
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start(); self.addCleanup(self.finish)

    def finish(self):
        self.server.shutdown(); self.server.server_close(); self.thread.join(timeout=2)
        self.redirect.__exit__(None, None, None)
        self.assertFalse(self.thread.is_alive())
        for value in list(self.credentials.values()) + list(self.provider.slots.values()):
            token = value['token'] if isinstance(value, dict) else str(value)
            self.assertNotIn(token, self.log.getvalue())

    def request(self, method, route, token=None, body=None, status=200):
        headers = {} if token is None else {'Authorization': 'Bearer ' + token}
        req = Request(self.base + route, method=method, headers=headers, data=body)
        try:
            with urlopen(req, timeout=3) as response: code, raw = response.status, response.read()
        except HTTPError as error: code, raw = error.code, error.read()
        self.assertEqual(code, status)
        return json.loads(raw)

    def issued(self, slot='old-write/0', status=200):
        return self.request('POST', '/fixture/api-custody/' + slot,
                            self.credentials['admin_bearer'], b'', status)

    def introspect(self, token, status=200):
        form = {k: self.credentials[k] for k in ('client_id', 'client_secret')}
        form.update(token=token, token_type_hint='access_token')
        return self.request('POST', '/introspect', body=urlencode(form).encode(), status=status)

    def test_four_slots_are_immutable_and_use_independent_provider_auth(self):
        identifiers = set()
        for slot in SLOTS:
            value = self.issued(slot); token = value['access_token']
            self.assertTrue(self.issued(slot) == value)
            self.assertEqual(value['metadata'], metadata(token, OWNER, self.credentials['expires_at']))
            identifiers.add(value['metadata']['credential_id'])
            self.assertEqual(self.request('GET', '/userinfo', token)['sub'], OWNER)
            evidence = self.introspect(token)
            self.assertTrue(evidence['active']); self.assertEqual(evidence['exp'], self.credentials['expires_at'])
            self.introspect(token[:-1] + ('A' if token[-1] != 'A' else 'B'), 401)
        self.assertEqual(len(identifiers), 4); self.assertEqual(len(self.provider.slots), 4)
        self.assertTrue(self.request('GET', '/fixture/health')['ok'])

    def test_unissued_payload_and_foreign_control_are_refused(self):
        for actor in (None, self.credentials['bearer'], self.credentials['outsider_bearer']):
            self.request('POST', '/fixture/api-custody/old-write/0', actor, b'', 403)
        for slot in ('old-write/2', 'old-write/0?x=1', 'foreign/0', 'new-write/-1'):
            self.issued(slot, 400)
        self.request('POST', '/fixture/api-custody/old-write/0', self.credentials['admin_bearer'], b'{}', 400)
        from migration_api_custody_provider import issue
        self.introspect(issue(OWNER, self.credentials['expires_at']), 401)
        self.assertEqual(self.provider.slots, {})

    def test_expiry_cannot_be_extended_by_mutating_provider_config(self):
        token = self.issued()['access_token']
        self.credentials['expires_at'] += 60
        self.issued(status=409)
        self.introspect(token, 401)

    def test_expired_credentials_do_not_authenticate_or_reissue(self):
        token = self.issued()['access_token']
        with patch('migration_api_custody_provider.time.time', return_value=self.credentials['expires_at'] + 1):
            self.issued(status=409)
            self.introspect(token, 401)
        for bad in (True, None, 0, int(time.time()) + 9000):
            self.credentials['expires_at'] = bad; self.issued(status=409)

    def test_client_fetch_keeps_token_out_of_application_recorder(self):
        fixture = SimpleNamespace(admin_bearer=self.credentials['admin_bearer'], requests=[])
        with patch.dict(os.environ, {'MATRIX_IDENTITY_URL': self.base}):
            token, observed = fetch(fixture, 'old-write/0')
        self.assertEqual(observed, {'slot': 'old-write/0', **metadata(token, OWNER, self.credentials['expires_at'])})
        self.assertEqual(fixture.requests, [])
        for url in ('https://127.0.0.1', 'http://foreign', self.base + '?x=1', self.base + '/foreign'):
            with patch.dict(os.environ, {'MATRIX_IDENTITY_URL': url}), self.assertRaises(RuntimeError):
                fetch(fixture, 'old-write/0')
        with self.assertRaises(RuntimeError): fetch(fixture, 'other/0')

    def test_overlay_only_replaces_provider_code(self):
        objects = base_manifests('api', 'runtime', 'admin', 'key', 'mongo', {}, identity_credentials=self.credentials)
        before = deepcopy(objects); changed = manifests(objects)
        self.assertEqual(before, objects)
        for a,b in zip(before, changed):
            if a['metadata']['name'] not in ('identity', 'identity-code'): self.assertEqual(a,b)
        with self.assertRaises(ValueError): manifests(changed)
        next(o for o in objects if o['metadata']['name'] == 'api-env')['data']['CHRONO_AUTH_MODE'] = 'trusted'
        with self.assertRaises(ValueError): manifests(objects)


if __name__ == '__main__': unittest.main()
