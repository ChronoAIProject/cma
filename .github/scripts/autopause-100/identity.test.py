"""Prove deployment-specific credential minting at the fixture HTTP boundary."""
from datetime import datetime, timedelta, timezone
import http.client
from http.server import ThreadingHTTPServer
import importlib.util
import json
from pathlib import Path
import secrets
import threading
import unittest

SOURCE = Path(__file__).resolve().parent.parent / 'multi-am-features/owner/identity.py'
SPEC = importlib.util.spec_from_file_location('fixture_identity', SOURCE)
identity = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(identity)


class DeploymentMintTests(unittest.TestCase):
    def setUp(self):
        class Handler(identity.Identity):
            config = {'bearer': secrets.token_hex(32), 'admin_bearer': secrets.token_hex(32)}
            keys = {}
            codes = {}
            lock = threading.Lock()

        self.handler = Handler
        self.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever)
        self.thread.start()

    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=3)
        self.assertFalse(self.thread.is_alive())

    def mint(self, deployment):
        selected = ['fixture-llm', 'fixture-ornn']
        body = {'name': 'cma:' + deployment + ':sandbox-generation-1',
                'scopes': 'proxy read',
                'expires_at': (datetime.now(timezone.utc) + timedelta(hours=1)).isoformat(),
                'allowed_service_ids': selected, 'allowed_node_ids': ['fixture-node'],
                'allow_all_services': False, 'allow_all_nodes': False,
                'allow_auto_connected_services': False,
                'scope_plan_digest': identity.scope_plan(selected)['normalized_grant_digest']}
        connection = http.client.HTTPConnection(*self.server.server_address, timeout=3)
        try:
            connection.request('POST', '/api/v1/api-keys', json.dumps(body),
                {'Content-Type': 'application/json',
                 'Authorization': 'Bearer ' + self.handler.config['bearer']})
            response = connection.getresponse()
            return response.status, json.loads(response.read())
        finally:
            connection.close()

    def test_selected_deployment_mints_one_exactly_scoped_key(self):
        self.handler.config['deployment_id'] = 'autopause18-build02'
        status, value = self.mint('autopause18-build02')
        self.assertEqual(status, 200)
        self.assertEqual(value['allowed_service_ids'], ['fixture-llm', 'fixture-ornn'])
        self.assertEqual(len(self.handler.keys), 1)

    def test_foreign_and_regex_lookalike_deployments_are_refused_without_a_key(self):
        self.handler.config['deployment_id'] = 'autopause.fixture'
        for deployment in ['native-am-1115', 'autopauseXfixture']:
            with self.subTest(deployment=deployment):
                status, value = self.mint(deployment)
                self.assertEqual(status, 400)
                self.assertEqual(value['error'], 'fixture_contract_refused')
                self.assertEqual(self.handler.keys, {})

    def test_legacy_unconfigured_fixture_keeps_its_original_contract(self):
        self.assertEqual(self.mint('native-am-1115')[0], 200)


if __name__ == '__main__':
    unittest.main()
