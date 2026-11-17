#!/usr/bin/env python3
"""Exercise the synthetic dependency over HTTP, including refused mutations."""
from contextlib import redirect_stdout
from copy import deepcopy
from datetime import datetime, timedelta, timezone
import hashlib
from http.server import ThreadingHTTPServer
import importlib.util
import io
import json
from pathlib import Path
import re
import secrets
import threading
import unittest
from urllib.error import HTTPError
from urllib.request import Request, urlopen
import uuid

from migration_bot_gateway import ContractError, Gateway, PUBLIC
from migration_bot_provider import handler, manifests


def policy():
    return {'schema_version': 1, 'name': 'Matrix', 'record_id': str(uuid.uuid4()),
            'profile': {'version': 1, 'nested': {'retained': ['one', 'two']}},
            'connection_ref': 'matrix-old-write-active', 'provider': {'slug': 'cma_codex'},
            'sources': [], 'connection_admission': {'type': 'all'},
            'partition': {'version': 1, 'key': 'conversation_and_sender'},
            'reply': {'sinks': [{'type': 'source'}]},
            'credentials': {'agent_key': secrets.token_urlsafe(32)}}


class GatewayTests(unittest.TestCase):
    def test_lifecycle_preserves_profile_and_never_returns_full_credentials(self):
        gateway = Gateway('fixture-owner')
        body = policy()
        code, created = gateway.request('POST', '/v1/channels', body)
        self.assertEqual(code, 201)
        self.assertNotIn(body['credentials']['agent_key'], json.dumps(created))
        self.assertEqual(created['definition']['profile'], body['profile'])
        self.assertEqual(created['definition']['secrets']['agent_key_hash'],
                         'sha256:' + hashlib.sha256(body['credentials']['agent_key'].encode()).hexdigest())
        identifier = created['channel_id']
        route = '/v1/channels/' + identifier
        self.assertEqual(gateway.request('GET', '/v1/channel-records/' + body['record_id']), (200, created))
        self.assertEqual(gateway.request('GET', route), (200, created))
        self.assertEqual(gateway.request('POST', '/v1/channels', body)[0], 409)
        update = {**body, 'expected_version': 1}
        del update['credentials']
        code, changed = gateway.request('PUT', route, update)
        self.assertEqual((code, changed['version']), (200, 2))
        self.assertEqual(changed['definition'], created['definition'])
        update['expected_version'] = 2
        update['credentials'] = {'agent_key': secrets.token_urlsafe(32)}
        code, rotated = gateway.request('PUT', route, update)
        self.assertEqual((code, rotated['version']), (200, 3))
        self.assertNotEqual(rotated['definition']['secrets'], created['definition']['secrets'])
        self.assertEqual(gateway.request('POST', route + '/test')[0], 200)
        before = gateway.summary()
        self.assertEqual(gateway.request('PUT', route, update)[0], 409)
        self.assertEqual(gateway.request('DELETE', route + '?expected_version=2')[0], 409)
        self.assertEqual(gateway.summary(), before)
        self.assertEqual(gateway.request('DELETE', route + '?expected_version=3'), (204, None))
        self.assertEqual(gateway.request('GET', '/v1/channel-records/' + body['record_id'])[0], 404)
        self.assertEqual(gateway.request('GET', route)[0], 404)
        self.assertEqual(gateway.summary()['effects'], {'create': 1, 'update': 2, 'delete': 1, 'test': 1})

    def test_refused_profile_retains_a_channel_for_actual_application_cleanup(self):
        gateway = Gateway('fixture-owner')
        body = policy()
        body['connection_ref'] = 'matrix-new-write-failed'
        code, row = gateway.request('POST', '/v1/channels', body)
        self.assertEqual(code, 201)
        self.assertEqual(row['provider']['refused'], [{'field': 'fixture_admission'}])
        self.assertEqual(gateway.summary()['channels'], 1)

    def test_invalid_policy_or_binding_never_mutates(self):
        gateway = Gateway('fixture-owner')
        body = policy()
        _, created = gateway.request('POST', '/v1/channels', body)
        for key, value in [('schema_version', 9), ('connection_ref', 'foreign'), ('profile', {}),
                           ('provider', {'slug': 'foreign'}), ('sources', [{'type': 'nyxid_relay'}]),
                           ('partition', {}), ('reply', {}), ('record_id', '../outside'),
                           ('credentials', {'agent_key': 'short'})]:
            with self.subTest(key=key):
                invalid = {**body, 'record_id': str(uuid.uuid4()), key: value}
                before = gateway.summary()
                with self.assertRaises(ContractError):
                    gateway.request('POST', '/v1/channels', invalid)
                self.assertEqual(gateway.summary(), before)
        changed = {**body, 'profile': {'changed': True}, 'expected_version': 1}
        with self.assertRaisesRegex(ContractError, 'changed_binding'):
            gateway.request('PUT', '/v1/channels/' + created['channel_id'], changed)
        for method, route in [('GET', '/foreign'), ('PATCH', '/v1/channels/' + created['channel_id']),
                              ('GET', '/v1/channel-connections/foreign'),
                              ('GET', '/v1/channel-records/x?extra=1'),
                              ('GET', '/v1/channels/' + created['channel_id'] + '?extra=1'),
                              ('POST', '/v1/channels?extra=1'),
                              ('DELETE', '/v1/channels/' + created['channel_id']),
                              ('GET', '/v1/channels/' + created['channel_id'] + '/test')]:
            with self.subTest(route=route), self.assertRaises(ContractError):
                gateway.request(method, route, body)
        self.assertEqual(gateway.request('GET', '/v1/channel-connections/matrix-old-write-active')[0], 200)


class ProviderHTTPTests(unittest.TestCase):
    def setUp(self):
        source = Path(__file__).with_name('multi-am-features') / 'owner' / 'identity.py'
        spec = importlib.util.spec_from_file_location('matrix_test_identity', source)
        self.identity = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.identity)
        self.handler = handler(self.identity)
        self.handler.config = {k: secrets.token_urlsafe(32) for k in ['bearer', 'admin_bearer', 'outsider_bearer']}
        self.reply_finished = threading.Event()
        reply = self.handler.reply
        completed = self.reply_finished
        def recorded_reply(provider, *args):
            try:
                return reply(provider, *args)
            finally:
                completed.set()
        self.handler.reply = recorded_reply
        self.server = ThreadingHTTPServer(('127.0.0.1', 0), self.handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.log = io.StringIO()
        self.redirect = redirect_stdout(self.log)
        self.redirect.__enter__()
        self.thread.start()

    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(5)
        self.redirect.__exit__(None, None, None)
        self.assertFalse(self.thread.is_alive())

    def request(self, method, path, body=None, actor='bearer', extra=None):
        self.reply_finished.clear()
        headers = {'Authorization': 'Bearer ' + self.handler.config[actor], 'Content-Type': 'application/json'}
        headers.update(extra or {})
        req = Request('http://127.0.0.1:' + str(self.server.server_port) + path, method=method,
                      data=None if body is None else json.dumps(body).encode(), headers=headers)
        try:
            response = urlopen(req, timeout=5)
        except HTTPError as error:
            response = error
        with response:
            self.assertEqual(response.headers['cmaeg-protocol-version'], '1')
            data = response.read()
            # HTTP 204 completes at the headers; the handler logs afterward.
            self.assertTrue(self.reply_finished.wait(5), 'response handler did not finish logging')
            return response.status, json.loads(data) if data else None

    def mint_body(self):
        allowed = ['fixture-llm']
        return {'name': 'cma:bot:bot_' + 'a' * 32 + ':agent:1', 'scopes': 'proxy read',
                'expires_at': (datetime.now(timezone.utc) + timedelta(days=7)).isoformat(),
                'allowed_service_ids': allowed, 'allowed_node_ids': ['fixture-node'],
                'allow_all_services': False, 'allow_all_nodes': False, 'allow_auto_connected_services': False,
                'scope_plan_digest': self.identity.scope_plan(allowed)['normalized_grant_digest']}

    def test_key_issuance_metadata_revocation_and_no_plaintext_logs(self):
        code, services = self.request('GET', '/api/v1/user-services')
        self.assertEqual(code, 200)
        self.assertTrue(all(row['credential_source'] == {'type': 'personal'} for row in services))
        code, minted = self.request('POST', '/api/v1/api-keys', self.mint_body())
        self.assertEqual(code, 200)
        # Both selected images call KeyIssuer::provided after metadata readback.
        # A generic test bearer can pass minting yet fail this real Bot contract.
        self.assertTrue(re.fullmatch(r'(?:nyx_|nyxid_ag_)[A-Za-z0-9_-]{16,128}', minted['full_key'])
                        is not None, 'issued key violates the application credential grammar')
        route = '/api/v1/api-keys/' + minted['id']
        _, saved = self.request('GET', route)
        self.assertNotIn('full_key', saved)
        self.assertEqual({k: v for k, v in minted.items() if k != 'full_key'}, saved)
        self.assertEqual(self.request('GET', '/api/v1/api-keys')[1], {'keys': [saved]})
        self.assertEqual(self.request('GET', '/api/v1/channel-conversations')[1]['total'], 0)
        self.assertEqual(self.request('DELETE', route)[0], 200)
        self.assertEqual(self.request('DELETE', route)[0], 404)
        self.assertEqual(self.request('GET', route)[0], 404)
        _, stats = self.request('GET', '/fixture/bot-state', actor='admin_bearer')
        self.assertEqual(stats['key_effects'], {'mint': 1, 'revoke': 1})
        self.assertEqual(stats['keys'], 0)
        for value in [*self.handler.config.values(), minted['full_key']]:
            self.assertNotIn(value, self.log.getvalue())

    def test_authority_and_invalid_mint_are_refused_without_effects(self):
        for method, route in [('GET', '/api/v1/user-services'), ('GET', '/api/v1/api-keys'),
                              ('GET', '/api/v1/channel-conversations'), ('POST', '/api/v1/api-keys'),
                              ('DELETE', '/api/v1/api-keys/nope'), ('GET', '/fixture/bot-state'),
                              ('GET', '/v1/channel-connections/matrix-old-write-active')]:
            self.assertIn(self.request(method, route, actor='outsider_bearer')[0], (401, 403))
        for changes in [{'name': 'foreign'}, {'allow_all_services': True}, {'scope_plan_digest': 'wrong'},
                        {'extra': True}, {'allowed_node_ids': []}, {'expires_at': 'invalid'},
                        {'allowed_service_ids': []}]:
            self.assertEqual(self.request('POST', '/api/v1/api-keys', {**self.mint_body(), **changes})[0], 400)
        self.assertEqual(self.request('GET', '/fixture/bot-state', actor='admin_bearer')[1]['keys'], 0)
        self.assertEqual(self.request('DELETE', '/api/v1/api-keys/nope')[0], 404)

    def test_gateway_http_and_parent_identity_routes(self):
        self.assertEqual(self.request('GET', '/fixture/health')[0], 200)
        self.assertEqual(self.request('POST', '/api/v1/api-keys/scope-plan', {'selected_service_ids': ['fixture-llm']})[0], 200)
        body = policy()
        self.assertEqual(self.request('POST', '/v1/channels', body, extra={'X-CMAEG-Owner': 'foreign'})[0], 403)
        self.assertEqual(self.request('POST', '/v1/channels', {})[0], 400)
        _, row = self.request('POST', '/v1/channels', body)
        route = '/v1/channels/' + row['channel_id']
        self.assertEqual(self.request('PUT', route, {**body, 'expected_version': 1})[0], 200)
        self.assertEqual(self.request('DELETE', route + '?expected_version=2')[0], 204)
        deletes = [json.loads(line) for line in self.log.getvalue().splitlines()
                   if json.loads(line)['method'] == 'DELETE']
        self.assertEqual(deletes[-1]['route'], '/v1/channels/:id')


class ManifestTests(unittest.TestCase):
    def objects(self):
        return [{'kind': 'ConfigMap', 'metadata': {'name': 'api-env'}, 'data': {'CHRONO_AUTH_MODE': 'provider'}},
                {'kind': 'ConfigMap', 'metadata': {'name': 'identity-code'}, 'data': {'identity.py': 'retained'}},
                {'kind': 'Deployment', 'metadata': {'name': 'identity'}, 'spec': {'template': {'spec': {
                    'containers': [{'command': ['/usr/local/bin/python3', '-B', '/code/identity.py']}]}}}}]

    def test_manifest_preserves_base_identity_and_uses_only_local_dependency(self):
        objects = self.objects()
        self.assertIs(manifests(objects), objects)
        self.assertEqual(objects[0]['data']['CHRONO_CMAEG_BASE_URL'], 'http://identity:8080')
        self.assertEqual(objects[1]['data']['identity.py'], 'retained')
        self.assertEqual(objects[0]['data']['CHRONO_CMAEG_PUBLIC_URL'], PUBLIC)
        self.assertEqual(len(objects[1]['data']), 3)
        objects[0]['data']['CHRONO_AUTH_MODE'] = 'trusted'
        with self.assertRaises(ContractError):
            manifests(objects)
        objects = self.objects()
        objects[2]['spec']['template']['spec']['containers'][0]['command'] = ['foreign']
        with self.assertRaises(ContractError):
            manifests(objects)


if __name__ == '__main__':
    unittest.main()
