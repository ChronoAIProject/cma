"""Full native render: exact private inputs, narrow authority, honest evidence."""
from copy import deepcopy
import json
import unittest
from unittest.mock import patch

from migration_native_overlay import render, select, NETWORK_POLICY, APP_RULES, ROLE_NAME
from migration_native_test_fixture import NODE, arguments, fixture, synthetic_tls


class NativeOverlayTests(unittest.TestCase):
    def setUp(self):
        self.objects = fixture()
        tls, pins = synthetic_tls()
        self.addCleanup(pins.stop)
        pins.start()
        self.args = arguments(tls)

    def test_default_off_has_no_read_or_mutation(self):
        with patch('migration_native_overlay.dependencies') as load:
            result, contract = render(self.objects)
        self.assertIs(result, self.objects)
        self.assertFalse(contract['enabled'])
        load.assert_not_called()
        for flag in (None, 1, 'true'):
            with self.assertRaisesRegex(ValueError, 'explicit_opt_in'):
                render(self.objects, enabled=flag)

    def test_actual_render_preserves_custody_and_binds_all_resources(self):
        original = deepcopy(self.objects)
        rendered, contract = render(self.objects, **self.args)
        self.assertEqual(self.objects, original)
        self.assertEqual((rendered, contract), render(self.objects, **self.args))
        for kind, name in [('ConfigMap', 'mongo-init'), ('Deployment', 'mongo'),
                           ('Job', 'mongo-init'), ('ConfigMap', 'identity-code'), ('Deployment', 'identity')]:
            self.assertEqual(select(rendered, kind, name), select(original, kind, name))
        before = select(original, 'ConfigMap', 'api-env')['data']
        after = select(rendered, 'ConfigMap', 'api-env')['data']
        for key in before:
            if key.startswith(('CHRONO_AUTH_', 'CHRONO_NYXID_', 'CHRONO_MONGODB_', 'CHRONO_USAGE_')):
                self.assertEqual(before[key], after[key], key)
        self.assertEqual(after['CHRONO_SANDBOX_IMAGE'], self.args['runtime']['image'])
        self.assertEqual(after['CHRONO_AGENT_SANDBOX_RUNTIME_TRANSPORT'], 'direct')
        self.assertEqual(after['CHRONO_AGENT_SANDBOX_HOME_MODE'], 'native_vct_full_home_v2')
        self.assertEqual(after['CHRONO_AGENT_SANDBOX_HOME_STORAGE_CLASS'], 'cma-native-rwop')
        self.assertEqual(after['CHRONO_AGENT_SANDBOX_HOME_CAPACITY_GIB'], '4')
        self.assertEqual(json.loads(after['CHRONO_AGENT_SANDBOX_TEMPLATE_NETWORK_POLICY']), NETWORK_POLICY)
        for key in after:
            if key.endswith(('_PREPARED_TARGET', '_WARM_POOL_TARGET')):
                self.assertEqual(after[key], '0')
        self.assertEqual(select(rendered, 'Role', ROLE_NAME)['rules'], APP_RULES)
        self.assertFalse(select(rendered, 'ServiceAccount', 'cma-runtime')['automountServiceAccountToken'])
        spec = select(rendered, 'Deployment', 'api')['spec']['template']['spec']
        self.assertEqual(spec['serviceAccountName'], 'cma-migration-am')
        self.assertEqual(spec['nodeSelector'], {'kubernetes.io/hostname': NODE})
        self.assertEqual(spec['containers'][0]['startupProbe']['httpGet']['path'], '/livez')
        self.assertEqual({e['name'] for e in spec['containers'][0]['env']}, {
            'CHRONO_AGENT_MANAGER_POD_NAME', 'CHRONO_AGENT_MANAGER_POD_NAMESPACE', 'CHRONO_AGENT_MANAGER_POD_UID'})
        native = [obj for obj in rendered if obj['kind'] == 'CustomResourceDefinition'
                  and obj['spec']['group'] in ('agents.x-k8s.io', 'extensions.agents.x-k8s.io')]
        self.assertEqual(len(native), 4)
        for obj in native:
            self.assertIn('spec', obj['spec']['versions'][0]['schema']['openAPIV3Schema']['properties'])
        self.assertEqual(contract['runtime_status'], 'NOT_RUN')
        self.assertEqual(contract['compatibility'], 'NOT_PROVEN')
        self.assertEqual(contract['endpoints']['tls_verification'], 'REQUIRED_AT_RUNTIME')
        evidence = json.dumps(contract)
        for sensitive in (self.args['provider_control'], *self.args['tls'].values(),
                          before['CHRONO_AUTH_ENCRYPTION_KEY'], before['CHRONO_MONGODB_URI'],
                          before['CHRONO_NYXID_CLIENT_SECRET']):
            self.assertNotIn(sensitive, evidence)

    def test_secret_or_excess_authority_rejected(self):
        extra = [
            {'apiVersion': 'v1', 'kind': 'Secret', 'metadata': {'name': 'forbidden', 'namespace': 'cma-migration'}},
            {'apiVersion': 'rbac.authorization.k8s.io/v1', 'kind': 'ClusterRole',
             'metadata': {'name': 'forbidden', 'namespace': 'cma-migration'}},
            {'apiVersion': 'v1', 'kind': 'ServiceAccount',
             'metadata': {'name': 'foreign', 'namespace': 'cma-migration'}},
            {'apiVersion': 'rbac.authorization.k8s.io/v1', 'kind': 'Role',
             'metadata': {'name': 'foreign', 'namespace': 'cma-migration'}},
        ]
        for obj in extra:
            with self.subTest(kind=obj['kind']), self.assertRaises(ValueError):
                render([*self.objects, obj], **self.args)
        for resource in ('*', 'secrets', 'pods/exec'):
            changed = deepcopy(self.objects)
            select(changed, 'Role', ROLE_NAME)['rules'][0]['resources'].append(resource)
            with self.assertRaisesRegex(ValueError, 'role_drift'):
                render(changed, **self.args)
        changed = deepcopy(self.objects)
        select(changed, 'Deployment', 'api')['spec']['template']['spec']['imagePullSecrets'] = [{'name': 'x'}]
        with self.assertRaisesRegex(ValueError, 'secret_authority'):
            render(changed, **self.args)
        foreign = {'apiVersion': 'v1', 'kind': 'Pod',
                   'metadata': {'name': 'injected', 'namespace': 'cma-migration'}}
        with self.assertRaisesRegex(ValueError, 'resource_set'):
            render([*self.objects, foreign], **self.args)

    def test_foreign_identity_duplicates_and_missing_objects_fail(self):
        for kind, name in [('Namespace', 'cma-migration'), ('ConfigMap', 'api-env'),
                           ('Role', ROLE_NAME), ('RoleBinding', ROLE_NAME), ('Deployment', 'api')]:
            obj = select(self.objects, kind, name)
            for changed in ([*self.objects, deepcopy(obj)], [x for x in self.objects if x is not obj]):
                with self.subTest(kind=kind), self.assertRaises(ValueError):
                    render(changed, **self.args)
        for kind, name in [('Namespace', 'cma-migration'), ('ConfigMap', 'api-env')]:
            changed = deepcopy(self.objects)
            select(changed, kind, name)['metadata']['namespace'] = 'foreign'
            with self.assertRaisesRegex(ValueError, 'namespace_mismatch'):
                render(changed, **self.args)
        changed = deepcopy(self.objects)
        select(changed, 'Namespace', 'cma-migration')['spec'] = {'finalizers': ['x']}
        with self.assertRaisesRegex(ValueError, 'namespace_spec'):
            render(changed, **self.args)
        changed = deepcopy(self.objects)
        select(changed, 'RoleBinding', ROLE_NAME)['subjects'][0]['name'] = 'foreign'
        with self.assertRaisesRegex(ValueError, 'binding_mismatch'):
            render(changed, **self.args)

    def test_template_drift_and_reader_overrides_fail(self):
        changes = [
            ('env', 'CHRONO_AUTH_MODE', 'trusted', 'provider_auth'),
            ('env', 'CHRONO_AGENT_SANDBOX_TEMPLATE_NETWORK_POLICY', '{}', 'template_policy'),
            ('env', 'CHRONO_AGENT_SANDBOX_API_SERVER', 'https://foreign.invalid', 'foreign_runtime'),
            ('env', 'CHRONO_AGENT_SANDBOX_IMAGE_PULL_SECRETS', 'forbidden', 'foreign_runtime'),
            ('deployment', 'replicas', 2, 'single_reader'),
            ('deployment', 'strategy', {'type': 'RollingUpdate'}, 'single_reader'),
            ('spec', 'hostNetwork', True, 'container_contract'),
            ('spec', 'initContainers', [{'name': 'x'}], 'container_contract'),
            ('container', 'name', 'not-api', 'container_contract'),
            ('container', 'image', 'api:latest', 'immutable_image'),
            ('container', 'imagePullPolicy', 'Always', 'immutable_image'),
            ('container', 'env', [{'name': 'CHRONO_MONGODB_URI', 'value': 'foreign'}], 'environment_override'),
            ('container', 'envFrom', [{'configMapRef': {'name': 'foreign'}}], 'environment_override'),
        ]
        for scope, key, value, error in changes:
            changed = deepcopy(self.objects)
            deployment = select(changed, 'Deployment', 'api')['spec']
            spec = deployment['template']['spec']
            target = {'env': select(changed, 'ConfigMap', 'api-env')['data'], 'deployment': deployment,
                      'spec': spec, 'container': spec['containers'][0]}[scope]
            target[key] = value
            before = deepcopy(changed)
            with self.subTest(key=key), self.assertRaisesRegex(ValueError, error):
                render(changed, **self.args)
            self.assertEqual(changed, before)


if __name__ == '__main__':
    unittest.main()
