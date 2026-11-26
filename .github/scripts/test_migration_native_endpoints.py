"""Local endpoints retain verified TLS and never expose control via a Service."""
from copy import deepcopy
import json
import secrets
import unittest
from unittest.mock import MagicMock, patch

from migration_native_endpoints import DNS_NAMES, bind_dns, render
from migration_native_provider_server import serve
from migration_native_test_fixture import CLUSTER, NODE, synthetic_tls


class NativeEndpointTests(unittest.TestCase):
    def setUp(self):
        self.tls, pins = synthetic_tls()
        self.addCleanup(pins.stop)
        pins.start()
        self.token = secrets.token_urlsafe(32)

    def test_tls_model_and_loopback_control_are_separate(self):
        objects, receipt = render(CLUSTER, NODE, self.tls, self.token)
        self.assertEqual(len(objects), 6)
        self.assertEqual(receipt['dns'], DNS_NAMES)
        self.assertEqual(receipt['runtime_status'], 'NOT_RUN')
        self.assertEqual(receipt['provider_control']['bind'], '127.0.0.1')
        self.assertFalse(receipt['provider_control']['service_exposed'])
        for obj in objects:
            self.assertEqual(obj['metadata']['namespace'], 'cma-migration')
            if obj['kind'] == 'Service':
                self.assertNotIn(8081, [p['port'] for p in obj['spec']['ports']])
            if obj['kind'] == 'Deployment':
                spec = obj['spec']['template']['spec']
                self.assertFalse(spec['automountServiceAccountToken'])
                self.assertEqual(spec['nodeSelector']['kubernetes.io/hostname'], NODE)
                self.assertEqual(spec['containers'][0]['imagePullPolicy'], 'Never')
        for value in (*self.tls.values(), self.token):
            self.assertNotIn(value, json.dumps(receipt))
        self.assertEqual((objects, receipt), render(CLUSTER, NODE, self.tls, self.token))

    def test_unbound_tls_and_control_material_rejected(self):
        for tls in (None, {}, {**self.tls, 'certificate': 'changed'},
                    {**self.tls, 'private_key': 3}, {**self.tls, 'extra': 'x'},
                    {**self.tls, 'certificate': 'x' * 65537}):
            with self.assertRaisesRegex(ValueError, 'pinned_tls'):
                render(CLUSTER, NODE, tls, self.token)
        for token in ('', 'short', 'x' * 257, 'a' * 32 + '\n', None):
            with self.assertRaisesRegex(ValueError, 'ephemeral_control'):
                render(CLUSTER, NODE, self.tls, token)

    def test_dns_candidate_preserves_cas_and_all_existing_plugins(self):
        original = {'apiVersion': 'v1', 'kind': 'ConfigMap', 'metadata': {
            'name': 'coredns', 'namespace': 'kube-system', 'uid': 'owned-uid', 'resourceVersion': '7'},
            'data': {'Corefile': '.:53 {\n    errors\n    kubernetes cluster.local\n    reload\n}\n',
                     'untouched': 'value'}}
        before = deepcopy(original)
        result = bind_dns(original, CLUSTER, NODE)
        self.assertEqual(original, before)
        self.assertEqual(result['metadata'], original['metadata'])
        self.assertEqual(result['data']['untouched'], 'value')
        self.assertIn('    kubernetes cluster.local\n    reload\n}', result['data']['Corefile'])
        for source, target in DNS_NAMES.items():
            self.assertIn(f'rewrite name exact {source} {target}', result['data']['Corefile'])
        with self.assertRaisesRegex(ValueError, 'template_drift'):
            bind_dns(result, CLUSTER, NODE)
        for key in ('uid', 'resourceVersion', 'namespace', 'name'):
            invalid = deepcopy(original)
            invalid['metadata'][key] = ''
            with self.assertRaisesRegex(ValueError, 'coredns_readback'):
                bind_dns(invalid, CLUSTER, NODE)
        for corefile in ('example:53 {\nreload\n}', '.:53 {\n}', '.:53 {\nreload\n}\n.:53 {\n}', None):
            invalid = deepcopy(original)
            invalid['data']['Corefile'] = corefile
            with self.assertRaisesRegex(ValueError, 'template_drift'):
                bind_dns(invalid, CLUSTER, NODE)
        for invalid in (None, {}, {'apiVersion': 'v1', 'kind': 'Secret'}):
            with self.assertRaisesRegex(ValueError, 'coredns_readback'):
                bind_dns(invalid, CLUSTER, NODE)

    def test_server_wraps_only_model_listener_and_joins_control(self):
        instances = []
        def factory(address, handler):
            server = MagicMock()
            server.__enter__.return_value = server
            instances.append((address, handler, server))
            return server
        with patch('migration_native_provider_server.ssl.SSLContext') as tls, \
                patch('migration_native_provider_server.threading.Thread') as worker:
            worker.return_value.is_alive.return_value = False
            serve('ephemeral-certificate-path', 'ephemeral-key-path', server_type=factory)
            tls.return_value.load_cert_chain.assert_called_once_with(
                'ephemeral-certificate-path', 'ephemeral-key-path')
            self.assertEqual(instances[0][0], ('127.0.0.1', 8081))
            self.assertEqual(instances[1][0], ('0.0.0.0', 8080))
            tls.return_value.wrap_socket.assert_called_once()
            self.assertTrue(tls.return_value.wrap_socket.call_args.kwargs['server_side'])
            self.assertIs(instances[1][2].socket, tls.return_value.wrap_socket.return_value)
            instances[0][2].shutdown.assert_called_once()
            worker.return_value.join.assert_called_once_with(timeout=5)
        with patch('migration_native_provider_server.ssl.SSLContext'), \
                patch('migration_native_provider_server.threading.Thread') as worker:
            worker.return_value.is_alive.return_value = True
            with self.assertRaisesRegex(RuntimeError, 'not_joined'):
                serve('certificate', 'key', server_type=factory)


if __name__ == '__main__':
    unittest.main()
