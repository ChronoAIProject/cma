from copy import deepcopy
import json
import sys
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import Mock

sys.path.insert(0, str(Path(__file__).resolve().parent / 'owner'))
from gateway import DATABASE, configured_role, initialize_gateway


def role(name):
    return {'createRole': name, 'roles': [], 'privileges': [
        {'resource': {'db': DATABASE, 'collection': 'channels'}, 'actions': ['find']},
    ]}


class GatewayFixtureTests(unittest.TestCase):
    def test_generated_database_authority_cannot_inherit_or_cross_database_boundaries(self):
        expected = role('cma_eg_runtime')
        self.assertEqual(configured_role(expected, 'cma_eg_runtime'), expected)
        for change in ({'db': 'other-fixture', 'collection': 'channels'}, {'cluster': True}):
            invalid = deepcopy(expected)
            invalid['privileges'][0]['resource'] = change
            with self.assertRaisesRegex(ValueError, 'gateway_fixture_role_unbound'):
                configured_role(invalid, 'cma_eg_runtime')
        with self.assertRaisesRegex(ValueError, 'gateway_fixture_role_unbound'):
            configured_role(dict(expected, roles=['readWrite']), 'cma_eg_runtime')

    def test_schema_and_ready_process_use_same_image_and_exact_fixture_database(self):
        run = 'cma-ci-' + 'a' * 12
        image = 'localhost/multi-am-cmaeg@sha256:' + 'b' * 64
        client = Mock(namespace='cma-native-am')
        client.k.side_effect = [json.dumps(role('cma_eg_runtime')), json.dumps(role('cma_eg_migrator'))]
        client.kjson.side_effect = [
            {'status': {'phase': 'Succeeded'}}, {'status': {'phase': 'Succeeded'}},
            {'status': {'phase': 'Succeeded'}},
            {'spec': {'containers': [{'image': image}]}, 'status': {'phase': 'Running',
                'containerStatuses': [{'name': 'gateway', 'ready': True}]}},
        ]
        client.wait.side_effect = lambda predicate, _seconds, _code: self.assertTrue(predicate())
        client.forward.return_value = 'http://127.0.0.1:12345'
        client.http.side_effect = [(200, {'ready': True}), (200, {'platforms': [], 'relay_authoring': {}}),
                                   (403, {}), (401, {})]
        client.http_raw.return_value = (200, {'Content-Type': 'text/html'},
            b'<section id="integration"></section><section id="architecture"></section><section id="protocol"></section>')
        fixture = SimpleNamespace(client=client, mongo=Mock(return_value=True), mark_initialization_step=Mock(),
            record=Mock(), secret={'bearer': 'generated-owner', 'outsider_bearer': 'generated-outsider',
                                   'gateway_identity': {'client_id': 'generated-id', 'client_secret': 'generated-value'}},
            ready={'fixture': 'features', 'run_id': run, 'context': 'kind-' + run,
                'server': 'https://127.0.0.1:23456', 'namespace': 'cma-native-am', 'expected_eg_image': image})

        self.assertEqual(initialize_gateway(fixture), client.forward.return_value)
        objects = [obj for call in client.apply.call_args_list for obj in call.args[0]]
        pods = [obj['spec']['containers'][0] for obj in objects if obj['kind'] == 'Pod']
        self.assertEqual([pod.get('args', []) for pod in pods],
                         [['--print-runtime-role'], ['--print-migration-role'], ['--migrate'], []])
        self.assertTrue(all(pod['image'] == image and pod['imagePullPolicy'] == 'Never' for pod in pods))
        self.assertEqual({obj['kind'] for obj in objects}, {'Pod', 'ConfigMap'})
        self.assertIn('gateway.getCollectionNames().length !== 0', fixture.mongo.call_args.args[0])
        client.http_raw.assert_called_once_with(client.forward.return_value, 'GET', '/docs')
        self.assertEqual(fixture.record.call_args.kwargs['manifest'], image.split('@', 1)[1])


if __name__ == '__main__':
    unittest.main()
