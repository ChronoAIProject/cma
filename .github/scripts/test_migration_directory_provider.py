"""Loopback checks for profile replacement, token evidence and scoped control."""
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
from urllib.parse import urlencode
from urllib.request import Request, urlopen

import migration_kind_runner as runner
from migration_directory_provider import NAMES, handler, manifests
from test_migration_kind_runner import IMAGE, MONGO, OLD


class DirectoryProviderTests(unittest.TestCase):
    def setUp(self):
        source = Path(__file__).with_name('multi-am-features')/'owner'/'identity.py'
        spec = importlib.util.spec_from_file_location('directory_test_identity', source)
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

    def request(self, method, route, actor='admin_bearer', status=200, body=None):
        headers = {} if actor is None else {'Authorization': 'Bearer ' + self.credentials[actor]}
        request = Request('http://127.0.0.1:' + str(self.server.server_port) + route,
                          method=method, headers=headers, data=body)
        try:
            with urlopen(request, timeout=3) as response:
                code, raw = response.status, response.read()
        except HTTPError as error:
            code, raw = error.code, error.read()
        self.assertEqual(code, status)
        return json.loads(raw)

    def test_profile_switches_are_exact_and_do_not_mint_tokens(self):
        for stage, name in NAMES.items():
            state = self.request('POST', '/fixture/directory/' + stage)
            self.assertEqual(state, {'profile_stage': stage, 'token_responses': 0})
            self.assertEqual(self.request('GET', '/userinfo', 'bearer')['name'], name)
            self.assertEqual(self.request('GET', '/fixture/directory/state'), state)
        self.assertEqual(self.request('GET', '/fixture/health'), {'ok': True, 'native_provider': False})

    def test_owner_outsider_and_absent_credentials_cannot_control_fixture(self):
        for actor in ('bearer', 'outsider_bearer', None):
            for method, route in [('POST', '/fixture/directory/old-write'),
                                  ('GET', '/fixture/directory/state')]:
                self.request(method, route, actor, 403)
        self.request('GET', '/userinfo', None, 401)

    def test_invalid_control_path_or_payload_cannot_change_state(self):
        for suffix in ('foreign', 'old-write?x=1', 'old-write/', '%6fld-write'):
            self.request('POST', '/fixture/directory/' + suffix, status=400)
        self.request('POST', '/fixture/directory/old-write', status=400, body=b'{}')
        self.assertEqual(self.request('GET', '/fixture/directory/state')['profile_stage'], 'reader')

    def test_real_token_response_is_counted_without_logging_credentials(self):
        body = {key: self.credentials[key] for key in ('client_id', 'client_secret', 'refresh_token')}
        body['grant_type'] = 'refresh_token'
        self.request('POST', '/token', body=urlencode(body).encode())
        self.assertEqual(self.request('GET', '/fixture/directory/state')['token_responses'], 1)
        body['client_secret'] = 'invalid'
        self.request('POST', '/token', status=401, body=urlencode(body).encode())
        self.assertEqual(self.request('GET', '/fixture/directory/state')['token_responses'], 1)

    def test_manifest_keeps_images_auth_and_runtime_permissions_unchanged(self):
        original = runner.manifests(IMAGE, secrets.token_urlsafe(32), secrets.token_urlsafe(32),
            base64.b64encode(secrets.token_bytes(32)).decode(), MONGO,
            {'createRole': 'cma_am_runtime', 'roles': [], 'privileges': []},
            identity_credentials=self.credentials)
        before = deepcopy(original)
        changed = manifests(original)
        self.assertEqual(original, before)
        for old, new in zip(original, changed):
            if old['metadata']['name'] not in ('identity-code', 'identity'):
                self.assertEqual(old, new)
        self.assertFalse(any(obj['kind'] == 'Secret' for obj in changed))
        source = next(obj['data']['migration_directory_provider.py'] for obj in changed
                      if obj['metadata']['name'] == 'identity-code')
        namespace = {'__name__': 'injected_directory_provider'}
        exec(compile(source, 'migration_directory_provider.py', 'exec'), namespace)
        self.assertEqual(namespace['NAMES'], NAMES)
        for value in self.credentials.values():
            self.assertNotIn(str(value), source)
        with self.assertRaisesRegex(ValueError, 'already customized'):
            manifests(changed)
        config = next(obj for obj in original if obj['metadata']['name'] == 'api-env')
        config['data']['CHRONO_AUTH_MODE'] = 'trusted'
        with self.assertRaisesRegex(ValueError, 'provider authentication'):
            manifests(original)

    def test_runner_flag_selects_only_the_directory_dependency(self):
        args = ['--old-source', OLD, '--new-source', 'b'*40, '--old-image', IMAGE,
                '--new-image', IMAGE, '--fixture-command', 'python3 fixture.py',
                '--output', 'out', '--plan', 'plan.json', '--inventory', 'inventory.json',
                '--directory-fixture']
        with patch.object(runner, 'run', return_value={'outcome': 'PASS'}) as run:
            runner.main(args)
            self.assertTrue(run.call_args.args[0].directory_fixture)
        with patch('sys.stderr', io.StringIO()):
            with self.assertRaises(SystemExit) as error:
                runner.main(args + ['--share-fixture'])
            self.assertEqual(error.exception.code, 2)


if __name__ == '__main__':
    unittest.main()
