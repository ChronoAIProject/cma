"""Exercise the synthetic login redirect as an HTTP boundary."""
import contextlib
import http.client
from http.server import ThreadingHTTPServer
import io
import json
from pathlib import Path
import secrets
import sys
import threading
import time
import unittest
from urllib.parse import parse_qs, urlencode, urlsplit

sys.path.insert(0, str(Path(__file__).resolve().parent / 'owner'))
from identity import Identity, scope_plan


class IndependentActorTests(unittest.TestCase):
    def setUp(self):
        self.credentials = {key: secrets.token_urlsafe(32) for key in (
            'bearer', 'admin_bearer', 'outsider_bearer', 'refresh_token', 'client_id', 'client_secret')}
        self.credentials['expires_at'] = int(time.time()) + 3600
        self.credentials['deployment_id'] = 'cma-ci-123456789abc'
        class Handler(Identity):
            config = self.credentials
            keys = {}
        self.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.addCleanup(self.close)

    def close(self):
        self.server.shutdown(); self.server.server_close(); self.thread.join(timeout=5)
        self.assertFalse(self.thread.is_alive())

    def request(self, method, path, *, token=None, form=None, data=None):
        client = http.client.HTTPConnection(*self.server.server_address, timeout=5)
        try:
            with contextlib.redirect_stdout(io.StringIO()):
                body = urlencode(form) if form else json.dumps(data) if data else None
                client.request(method, path, body=body,
                               headers={'Authorization': 'Bearer ' + token} if token else {})
                response = client.getresponse(); body = json.loads(response.read())
                return response.status, body
        finally:
            client.close()

    def test_actors_verify_independently_and_outsider_gets_no_admin_authority(self):
        subjects = []
        for key in ['bearer', 'admin_bearer', 'outsider_bearer']:
            token = self.credentials[key]
            status, user = self.request('GET', '/userinfo', token=token)
            self.assertEqual(status, 200); subjects.append(user['sub'])
            status, observed = self.request('POST', '/introspect', form={
                'token': token, 'token_type_hint': 'access_token',
                'client_id': self.credentials['client_id'], 'client_secret': self.credentials['client_secret']})
            self.assertEqual(status, 200); self.assertEqual(observed['sub'], user['sub'])
            self.assertEqual(user['permissions'], observed['permissions'])
            self.assertEqual(any(x.startswith('cma:admin:') for x in observed['permissions']), key == 'admin_bearer')
        self.assertEqual(len(set(subjects)), 3)
        status, _ = self.request('GET', '/api/v1/services', token=self.credentials['outsider_bearer'])
        self.assertEqual(status, 401)

    def test_unknown_token_does_not_fall_back_to_owner(self):
        self.assertEqual(self.request('GET', '/userinfo', token='unknown-fixture-input')[0], 401)
        status, body = self.request('POST', '/introspect', form={
            'token': 'unknown-fixture-input', 'token_type_hint': 'access_token',
            'client_id': self.credentials['client_id'], 'client_secret': self.credentials['client_secret']})
        self.assertEqual(status, 401); self.assertFalse(body['active'])

    def test_organization_membership_is_current_and_bound_to_the_exact_actor(self):
        admin = self.credentials['admin_bearer']
        recipient = self.credentials['outsider_bearer']
        control = '/fixture/organization-membership'
        authorization = '/api/v1/orgs/fixture-organization/authorization'
        self.assertEqual(self.request('GET', '/api/v1/orgs', token=recipient), (200, {'orgs': []}))
        self.assertEqual(self.request('POST', control, token=recipient, data={'member': True})[0], 403)
        self.assertEqual(self.request('POST', control, token=admin, data={'member': 'true'})[0], 400)
        self.assertEqual(self.request('POST', control, token=admin, data={'member': True}), (200, {'member': True}))
        status, membership = self.request('GET', authorization, token=recipient)
        self.assertEqual(status, 200)
        self.assertEqual(membership['id'], 'fixture-organization')
        self.assertEqual(membership['your_role'], 'member')
        self.assertEqual(self.request('GET', authorization, token=admin)[0], 404)
        self.assertEqual(self.request('POST', control, token=admin, data={'member': False}), (200, {'member': False}))
        self.assertEqual(self.request('GET', authorization, token=recipient)[0], 404)
        self.assertEqual(self.request('GET', '/api/v1/orgs', token=recipient), (200, {'orgs': []}))

    def test_scope_key_uses_disposable_deployment_and_ornn_revocation(self):
        token = self.credentials['bearer']
        endpoint = '/api/v1/api-keys'
        selected = ['fixture-ornn']
        body = {'name': 'cma:another-deployment:fixture-sandbox', 'scopes': 'proxy read',
                'expires_at': '2099-01-01T00:00:00Z', 'allowed_service_ids': selected,
                'allowed_node_ids': ['fixture-node'], 'allow_all_services': False,
                'allow_all_nodes': False, 'allow_auto_connected_services': False,
                'scope_plan_digest': scope_plan(selected)['normalized_grant_digest']}
        self.assertEqual(self.request('POST', endpoint, token=token, data=body)[0], 400)
        body['name'] = 'cma:' + self.credentials['deployment_id'] + ':fixture-sandbox'
        status, created = self.request('POST', endpoint, token=token, data=body)
        self.assertEqual(status, 200)
        credential = created['full_key']
        self.assertEqual(self.request('GET', '/fixture/ornn-grant', token=credential), (200, {'granted': True}))
        self.assertEqual(self.request('GET', '/fixture/llm-grant', token=credential)[0], 401)
        self.assertEqual(self.request('GET', '/fixture/ornn-grant', token=token)[0], 401)
        self.assertEqual(self.request('DELETE', endpoint + '/' + created['id'], token=token)[0], 200)
        self.assertEqual(self.request('GET', '/fixture/ornn-grant', token=credential)[0], 401)
        selected = ['fixture-llm']
        body.update(allowed_service_ids=selected, scope_plan_digest=scope_plan(selected)['normalized_grant_digest'])
        status, created = self.request('POST', endpoint, token=token, data=body)
        self.assertEqual(status, 200)
        model_key = created['full_key']
        self.assertEqual(self.request('GET', '/fixture/llm-grant', token=model_key), (200, {'granted': True}))
        self.assertEqual(self.request('GET', '/fixture/ornn-grant', token=model_key)[0], 401)
        self.assertEqual(self.request('DELETE', endpoint + '/' + created['id'], token=token)[0], 200)
        self.assertEqual(self.request('GET', '/fixture/llm-grant', token=model_key)[0], 401)


class LoginRedirectTests(unittest.TestCase):
    def setUp(self):
        class Handler(Identity):
            config = {'client_id': 'test-client'}
            codes = {}
        self.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.addCleanup(self.close)

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=5)
        self.assertFalse(self.thread.is_alive())

    def authorize(self, **changes):
        params = {'client_id': 'test-client', 'redirect_uri': 'https://api.localhost.direct/auth/callback',
                  'response_type': 'code', 'code_challenge_method': 'S256',
                  'state': 'fixture-state', 'code_challenge': 'x' * 43, **changes}
        connection = http.client.HTTPConnection(*self.server.server_address, timeout=5)
        try:
            with contextlib.redirect_stdout(io.StringIO()):
                connection.request('GET', '/authorize?' + urlencode(params))
                response = connection.getresponse()
                body = response.read()
            return response.status, dict(response.getheaders()), body
        finally:
            connection.close()

    def test_state_is_encoded_and_callback_is_fixed(self):
        state = 'fixture\r\nLocation: https://unrelated.invalid'
        status, headers, _ = self.authorize(state=state)
        self.assertEqual(status, 302)
        callback = urlsplit(headers['Location'])
        self.assertEqual((callback.scheme, callback.netloc, callback.path),
                         ('https', 'api.localhost.direct', '/auth/callback'))
        self.assertEqual(parse_qs(callback.query)['state'], [state])
        self.assertNotIn('\r', headers['Location'])
        self.assertNotIn('\n', headers['Location'])

    def test_foreign_or_injected_destination_is_refused(self):
        for destination in ['https://unrelated.invalid/callback',
                            'https://api.localhost.direct/auth/callback\r\nX-Test: injected']:
            status, headers, _ = self.authorize(redirect_uri=destination)
            self.assertEqual(status, 400)
            self.assertNotIn('Location', headers)


if __name__ == '__main__':
    unittest.main()
