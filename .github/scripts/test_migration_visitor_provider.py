"""Visitor auth removes only native creation from the selected synthetic owner."""
from copy import deepcopy
from pathlib import Path
import importlib.util
import secrets
import unittest
from unittest.mock import patch

from migration_kind_manifests import manifests as base_manifests
from migration_visitor_provider import handler, manifests
import test_migration_launch_provider as wire


class VisitorProviderTests(unittest.TestCase):
    def test_owner_is_visitor_admin_unchanged_and_invalid_token_rejected(self):
        source = Path(__file__).with_name('multi-am-features') / 'owner/identity.py'
        spec = importlib.util.spec_from_file_location('visitor_test_identity', source)
        identity = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(identity)
        provider = handler(identity)
        instance = provider.__new__(provider)
        instance.config = {key: secrets.token_urlsafe(32) for key in (
            'bearer', 'admin_bearer', 'outsider_bearer')}
        before = deepcopy(identity.PERMISSIONS)
        owner, permissions = instance.actor(instance.config['bearer'])
        self.assertEqual(owner, identity.OWNER)
        self.assertEqual(permissions, [p for p in before if p != 'cma:sandbox:create'])
        self.assertIn('cma:trigger:launch', permissions)
        self.assertEqual(instance.actor(instance.config['admin_bearer'])[1], identity.ADMIN_PERMISSIONS)
        self.assertIsNone(instance.actor(secrets.token_urlsafe(32)))
        self.assertEqual(identity.PERMISSIONS, before)

    def test_overlay_selects_policy_and_readonly_authority_without_mutating_base(self):
        credentials = {key: secrets.token_urlsafe(32) for key in (
            'client_id', 'client_secret', 'bearer', 'admin_bearer', 'outsider_bearer', 'refresh_token')}
        credentials['expires_at'] = 2000000000
        base = base_manifests('api', 'runtime', 'admin', 'key', 'mongo', {}, identity_credentials=credentials)
        original = deepcopy(base)
        result = manifests(base)
        self.assertEqual(base, original)
        selected = {(o['kind'], o['metadata']['name']): o for o in result}
        self.assertEqual(selected[('ConfigMap', 'api-env')]['data']['CHRONO_TRIGGER_COMMAND_ADMISSION_ENABLED'], 'true')
        pod = selected[('Deployment', 'identity')]['spec']['template']['spec']
        self.assertEqual(pod['containers'][0]['command'][-1], '/code/migration_visitor_provider.py')
        code = selected[('ConfigMap', 'identity-code')]['data']
        self.assertIn('migration_launch_provider.py', code)
        roles = [o for o in result if o['kind'] == 'Role']
        self.assertTrue(all(rule['verbs'] == ['get', 'list'] for role in roles for rule in role['rules']))
        with self.assertRaisesRegex(ValueError, 'already selected'): manifests(result)
        identity = next(o for o in base if o['kind'] == 'Deployment' and o['metadata']['name'] == 'identity')
        identity['spec']['template']['spec']['containers'][0]['command'][-1] = '/foreign'
        with self.assertRaisesRegex(ValueError, 'already customized'): manifests(base)


class VisitorWireTests(unittest.TestCase):
    finish = wire.ProviderTests.finish
    request = wire.ProviderTests.request

    def setUp(self):
        with patch.object(wire, 'handler', handler):
            wire.ProviderTests.setUp(self)

    def test_requirements_provider_keeps_the_application_catalog_envelope(self):
        value = self.request('GET', '/api/v1/user-services')
        self.assertIsInstance(value, dict)
        self.assertEqual(value['services'], self.identity.SERVICES)
        catalog = self.request('GET', '/api/v1/services')
        self.assertIsInstance(catalog, dict)
        self.assertTrue(all(row['slug'] and row['name'] and row['service_type'] for row in catalog['services']))
        for row in self.identity.SERVICES:
            self.assertTrue(self.request('GET', '/api/v1/keys/' + row['id'])['connected'])
        for route in ('/api/v1/services', '/api/v1/user-services'):
            self.request('GET', route, actor=None, status=401)


if __name__ == '__main__':
    unittest.main()
