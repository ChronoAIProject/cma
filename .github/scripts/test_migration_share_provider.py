#!/usr/bin/env python3
"""Loopback HTTP checks for the scoped organization dependency."""
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
from unittest.mock import patch
from urllib.error import HTTPError
from urllib.request import Request, urlopen

import migration_kind_runner as runner
from migration_share_provider import ORGANIZATION, handler, manifests
from test_migration_kind_runner import IMAGE, MONGO, OLD


class ShareProviderTests(unittest.TestCase):
    def setUp(self):
        source = Path(__file__).with_name('multi-am-features')/'owner'/'identity.py'
        spec = importlib.util.spec_from_file_location('sharing_test_identity', source)
        identity = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(identity)
        self.provider = handler(identity)
        self.credentials = {key: secrets.token_urlsafe(32) for key in (
            'bearer', 'admin_bearer', 'outsider_bearer', 'client_id', 'client_secret', 'refresh_token')}
        self.credentials['expires_at'] = 2000000000
        self.provider.config = self.credentials
        self.server = ThreadingHTTPServer(('127.0.0.1', 0), self.provider)
        self.server.daemon_threads = True
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.log = io.StringIO()
        self.redirect = redirect_stdout(self.log)
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
            self.assertNotIn(str(value), self.log.getvalue())

    def get(self, route, credential='bearer', status=200):
        headers = {} if credential is None else {'Authorization': 'Bearer ' + self.credentials[credential]}
        request = Request('http://127.0.0.1:' + str(self.server.server_port) + route, headers=headers)
        try:
            with urlopen(request, timeout=3) as response:
                code, raw = response.status, response.read()
        except HTTPError as error:
            code, raw = error.code, error.read()
        self.assertEqual(code, status)
        return json.loads(raw)

    def test_exact_owner_membership_and_inventory_use_canonical_organization(self):
        row = self.get('/api/v1/orgs/' + ORGANIZATION + '/authorization')
        self.assertEqual(row, {'id': ORGANIZATION, 'your_role': 'admin', 'display_name': 'Matrix sharing'})
        self.assertEqual(self.get('/api/v1/orgs'), {'orgs': [row]})

    def test_admin_outsider_and_missing_authority_are_not_members(self):
        for actor in ('admin_bearer', 'outsider_bearer', None):
            for route in ('/api/v1/orgs', '/api/v1/orgs/' + ORGANIZATION + '/authorization'):
                with self.subTest(actor=actor, route=route):
                    self.assertEqual(self.get(route, actor, 403), {'error': 'not_member'})

    def test_unknown_or_ambiguous_organization_routes_are_refused(self):
        for suffix in ('/foreign/authorization', '/' + ORGANIZATION + '/authorization?extra=1',
                       '/' + ORGANIZATION + '/authorization/', '?extra=1', '/%2e%2e/authorization'):
            with self.subTest(suffix=suffix):
                self.assertEqual(self.get('/api/v1/orgs' + suffix, status=404),
                                 {'error': 'unknown_organization'})

    def test_existing_identity_routes_are_preserved(self):
        self.assertEqual(self.get('/fixture/health'), {'ok': True, 'native_provider': False})
        self.assertIn('sub', self.get('/userinfo'))

    def test_manifest_is_opt_in_read_only_and_keeps_credentials_ephemeral(self):
        objects = runner.manifests(IMAGE, secrets.token_urlsafe(32), secrets.token_urlsafe(32),
            base64.b64encode(secrets.token_bytes(32)).decode(), MONGO,
            {'createRole': 'cma_am_runtime', 'roles': [], 'privileges': []},
            identity_credentials=self.credentials)
        original = deepcopy(objects)
        changed = manifests(objects)
        self.assertEqual(objects, original)
        self.assertFalse(any(obj['kind'] == 'Secret' for obj in changed))
        roles = [obj for obj in changed if obj['kind'] == 'Role']
        self.assertTrue(all(rule['verbs'] == ['get', 'list'] for rule in roles[0]['rules']))
        source = next(obj['data']['migration_share_provider.py'] for obj in changed
                      if obj['metadata']['name'] == 'identity-code')
        namespace = {'__name__': 'injected_sharing_provider'}
        exec(compile(source, 'migration_share_provider.py', 'exec'), namespace)
        self.assertEqual(namespace['ORGANIZATION'], ORGANIZATION)
        for value in self.credentials.values():
            self.assertNotIn(str(value), source)
        with self.assertRaisesRegex(ValueError, 'already customized'):
            manifests(changed)

    def test_runner_flag_is_exclusive_and_selects_sharing_campaign(self):
        args = ['--old-source', OLD, '--new-source', 'b'*40, '--old-image', IMAGE,
                '--new-image', IMAGE, '--fixture-command', 'python3 fixture.py',
                '--output', 'out', '--plan', 'plan.json', '--inventory', 'inventory.json',
                '--share-fixture']
        with patch.object(runner, 'run', return_value={'outcome': 'PASS'}) as run:
            runner.main(args)
            self.assertTrue(run.call_args.args[0].share_fixture)
        with redirect_stdout(io.StringIO()), patch('sys.stderr', io.StringIO()):
            with self.assertRaises(SystemExit) as error:
                runner.main(args + ['--sandbox-admission-fixture'])
            self.assertEqual(error.exception.code, 2)


if __name__ == '__main__':
    unittest.main()
