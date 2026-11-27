#!/usr/bin/env python3
import base64
import copy
import hashlib
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import migration_kind_runner as runner


OLD = 'a' * 40
IMAGE = 'us-central1-docker.pkg.dev/example-project-01/example-registry-02/cma-api@sha256:' + '1' * 64
MONGO = 'docker.io/library/mongo@sha256:' + '3' * 64


def inspect(revision=OLD, image=IMAGE):
    return [{
        'Id': image.split('@', 1)[1].replace('sha256:', 'sha256:'),
        'Architecture': 'amd64', 'Os': 'linux',
        'RepoDigests': [image],
        'Descriptor': {'annotations': {'config.digest': 'sha256:' + '5' * 64}},
        'Config': {'Labels': {'org.opencontainers.image.revision': revision,
                              'org.opencontainers.image.source': 'https://github.com/ChronoAIProject/cma'}},
    }]


class MigrationKindRunnerTests(unittest.TestCase):
    def test_manifest_accepts_only_bounded_explicit_runtime_stack(self):
        args = (IMAGE, 'runtime-password', 'admin-password', 'key', MONGO, {})
        for stack_bytes in (None, 16777216):
            objects = runner.manifests(*args, runtime_stack_bytes=stack_bytes)
            values = next(value['data'] for value in objects if value['metadata']['name'] == 'api-env')
            self.assertEqual(values.get('RUST_MIN_STACK'), None if stack_bytes is None else str(stack_bytes))
        for stack_bytes in (True, '16777216', 1048575, 67108865):
            with self.subTest(stack_bytes=stack_bytes), self.assertRaisesRegex(ValueError, 'stack budget'):
                runner.manifests(*args, runtime_stack_bytes=stack_bytes)

    def test_identity_requires_exact_source_digest_platform_and_label(self):
        command = lambda argv: json.dumps(inspect())
        value = runner.checked_identity(command, IMAGE, OLD)
        self.assertEqual(value['source'], OLD)
        for image, source in [('cma-api:latest', OLD), (IMAGE, 'develop'), (IMAGE.replace('sha256:' + '1' * 64, 'sha256:' + '2' * 64), OLD)]:
            with self.subTest(image=image, source=source):
                with self.assertRaises(runner.RunnerError):
                    runner.checked_identity(command, image, source)

    def test_manifest_is_secret_free_and_switchable(self):
        objects = runner.manifests(IMAGE, 'runtime-password', 'admin-password',
                                   base64.b64encode(b'x' * 32).decode(), MONGO,
                                   {'createRole': 'cma_am_runtime', 'roles': [], 'privileges': []})
        self.assertFalse(any(value['kind'] == 'Secret' for value in objects))
        api = next(value for value in objects if value['kind'] == 'Deployment' and value['metadata']['name'] == 'api')
        self.assertEqual(api['spec']['template']['spec']['containers'][0]['imagePullPolicy'], 'Never')
        self.assertEqual(api['spec']['strategy']['type'], 'Recreate')
        self.assertEqual(next(value for value in objects if value['kind'] == 'Namespace')['metadata']['name'], runner.NAMESPACE)
        init = next(value for value in objects if value['kind'] == 'Job')
        self.assertIn('rs.initiate', next(value for value in objects if value['kind'] == 'ConfigMap' and value['metadata']['name'] == 'mongo-init')['data']['init.sh'])
        self.assertNotIn('pods/exec', json.dumps(objects))
        values = next(value['data'] for value in objects if value['metadata']['name'] == 'api-env')
        self.assertIn('authSource=matrix', values['CHRONO_MONGODB_URI'])
        self.assertNotIn('admin-password', json.dumps(values))
        mongo = next(value for value in objects if value['kind'] == 'Deployment' and value['metadata']['name'] == 'mongo')
        self.assertIn('--keyFile', mongo['spec']['template']['spec']['containers'][0]['command'][-1])
        data = next(value['data'] for value in objects if value['metadata']['name'] == 'mongo-init' and value['kind'] == 'ConfigMap')
        self.assertEqual(len(base64.b64decode(data['keyfile'], validate=True)), 48)
        crd = next(value for value in objects if value['kind'] == 'CustomResourceDefinition')
        self.assertEqual(crd['spec']['versions'][0]['name'], 'v1alpha1')

    def test_provider_manifest_uses_ephemeral_configmap_identity(self):
        credentials = {
            'client_id': 'matrix-client', 'client_secret': 'matrix-secret',
            'bearer': 'matrix-bearer', 'admin_bearer': 'matrix-admin',
            'outsider_bearer': 'matrix-outsider', 'refresh_token': 'matrix-refresh',
            'expires_at': 2000000000,
        }
        objects = runner.manifests(IMAGE, 'runtime-password', 'admin-password',
                                   base64.b64encode(b'x' * 32).decode(), MONGO,
                                   {'createRole': 'cma_am_runtime', 'roles': [], 'privileges': []},
                                   identity_credentials=credentials)
        self.assertFalse(any(value['kind'] == 'Secret' for value in objects))
        values = next(value['data'] for value in objects
                       if value['kind'] == 'ConfigMap' and value['metadata']['name'] == 'api-env')
        self.assertEqual(values['CHRONO_AUTH_MODE'], 'provider')
        self.assertEqual(values['CHRONO_NYXID_BASE_URL'], 'http://identity:8080')
        identity = next(value for value in objects
                        if value['kind'] == 'Deployment' and value['metadata']['name'] == 'identity')
        self.assertEqual(identity['spec']['template']['spec']['containers'][0]['imagePullPolicy'], 'Never')
        code = next(value for value in objects
                     if value['kind'] == 'ConfigMap' and value['metadata']['name'] == 'identity-code')
        self.assertIn('ThreadingHTTPServer', code['data']['identity.py'])
        self.assertIn('matrix-refresh', code['data']['identity.json'])

    def test_identity_dependency_is_pinned_reviewed_amd64_image(self):
        image = runner.IDENTITY
        command = lambda argv: json.dumps([{
            'Id': 'sha256:' + '4' * 64, 'Architecture': 'amd64', 'Os': 'linux',
            'RepoDigests': [image],
        }])
        value = runner.checked_identity_dependency(command, image)
        self.assertEqual(value['image'], image)
        with self.assertRaises(runner.RunnerError):
            runner.checked_identity_dependency(command, MONGO)

    def test_mongo_init_authenticates_same_database_and_waits_for_primary(self):
        script = runner.mongo_documents({'createRole': 'cma_am_runtime', 'roles': [], 'privileges': []})
        self.assertLess(script.index('rs.initiate'), script.index('createRole'))
        self.assertLess(script.index('rs.isMaster().ismaster'), script.index('createUser'))
        self.assertIn('pwd: process.env.MATRIX_RUNTIME_PASSWORD', script)
        self.assertIn('--authenticationDatabase matrix', script)

    def test_local_manifest_id_is_not_mislabeled_as_config_digest(self):
        value = runner.checked_identity(lambda argv: json.dumps(inspect()), IMAGE, OLD)
        self.assertEqual(value['local_image_id'], inspect()[0]['Id'])
        self.assertEqual(value['config'], 'sha256:' + '5' * 64)
        self.assertNotEqual(value['local_image_id'], value['config'])

    def test_dependency_requires_amd64_immutable_digest(self):
        command = lambda argv: json.dumps([{'Id': 'sha256:' + '4' * 64,
            'Architecture': 'amd64', 'Os': 'linux', 'RepoDigests': [MONGO]}])
        self.assertEqual(runner.checked_dependency(command, MONGO)['image'], MONGO)
        with self.assertRaises(runner.RunnerError):
            runner.checked_dependency(command, 'mongo:latest')

    def test_fixture_environment_contract_is_explicit(self):
        with tempfile.TemporaryDirectory() as directory:
            plan = Path(directory) / 'plan.json'
            plan.write_text(json.dumps({'schema': 'cma-migration-matrix-plan-v1'}))
            campaign = runner.Campaign(Path(directory), '/bin/false', 1, plan.read_bytes())
            self.assertEqual(campaign.cluster.startswith('cma-matrix-'), True)
            self.assertTrue(campaign.kubeconfig.name.endswith('.private'))
            self.assertEqual(campaign.plan_path.parent, Path(directory).resolve())
            self.assertNotEqual(campaign.plan_path, plan)
            self.assertEqual(campaign.plan_path.read_bytes(), plan.read_bytes())
            self.assertEqual(campaign.plan_sha256, hashlib.sha256(plan.read_bytes()).hexdigest())

    def test_generated_authentication_manifests_are_streamed_and_never_saved(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            plan = output / 'plan.json'
            plan.write_text('{}')
            campaign = runner.Campaign(output, 'kind-not-invoked', 1, plan.read_bytes())
            campaign.images = ['local-api', 'local-mongo']
            objects = runner.manifests('local-api', 'runtime-private-fixture',
                                       'admin-private-fixture', 'encryption-private-fixture',
                                       'local-mongo', {})
            calls = []

            def command(argv, **kwargs):
                calls.append((argv, kwargs))
                return ''

            with patch.object(campaign, 'command', side_effect=command), \
                    patch.object(campaign, 'reader_pods', return_value={
                        'metadata': {'resourceVersion': 'before-api'}, 'items': []}):
                campaign.create(objects)
            applies = [(argv, kwargs) for argv, kwargs in calls if 'apply' in argv]
            self.assertEqual(len(applies), 2)
            self.assertTrue(all(argv[-2:] == ['-f', '-'] for argv, _ in applies))
            infrastructure = json.loads(applies[0][1]['input_text'])
            self.assertEqual(infrastructure['kind'], 'List')
            self.assertFalse(any(obj['kind'] == 'Deployment' and obj['metadata']['name'] == 'api'
                                 for obj in infrastructure['items']))
            self.assertEqual(json.loads(applies[1][1]['input_text'])['metadata']['name'], 'api')
            self.assertIn('runtime-private-fixture', applies[0][1]['input_text'])
            for path in output.rglob('*'):
                if path.is_file():
                    content = path.read_text()
                    for value in ('runtime-private-fixture', 'admin-private-fixture',
                                  'encryption-private-fixture'):
                        self.assertNotIn(value, content)
            self.assertFalse((output / 'manifests.private').exists())
            self.assertFalse((output / 'api-deployment.private.json').exists())

    def test_fixture_receipt_requires_collections_shapes_and_all_checks(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'receipt.json'
            receipt = {'schema': 'cma-migration-fixture-v1', 'stage': 'old-write',
                       'source': OLD, 'image': IMAGE, 'collections': ['agents'],
                       'shapes': ['agent-envelope'], 'checks': {'write': True, 'read': True},
                       'samples': 3, 'outcome': 'PASS'}
            path.write_text(json.dumps(receipt))
            self.assertEqual(runner.require_fixture_observation(path, 'old-write', OLD, IMAGE)['samples'], 3)
            receipt['checks']['read'] = False
            path.write_text(json.dumps(receipt))
            with self.assertRaises(runner.RunnerError):
                runner.require_fixture_observation(path, 'old-write', OLD, IMAGE)

    def test_pod_binding_requires_one_ready_selected_image(self):
        pod = {'metadata': {'name': 'api-1', 'uid': 'uid-1'},
               'spec': {'containers': [{'name': 'api', 'image': 'cma-matrix-old'}]},
               'status': {'phase': 'Running', 'containerStatuses': [{
                   'name': 'api', 'ready': True, 'restartCount': 0,
                   'image': 'cma-matrix-old',
                   'imageID': 'docker-pullable://sha256:' + '4' * 64}]}}
        config = 'sha256:' + '4' * 64
        selected = runner.select_ready_api_pod({'items': [pod]}, 'cma-matrix-old', config)
        self.assertEqual(selected['uid'], 'uid-1')
        with self.assertRaises(RuntimeError):
            runner.select_ready_api_pod({'items': [pod, pod]}, 'cma-matrix-old', config)
        pod['status']['containerStatuses'][0]['image'] = 'cma-matrix-current'
        self.assertEqual(runner.select_ready_api_pod({'items': [pod]}, 'cma-matrix-old', config)['uid'], 'uid-1')
        with self.assertRaises(RuntimeError):
            runner.select_ready_api_pod({'items': [pod]}, 'cma-matrix-current', config)
        pod['status']['containerStatuses'][0]['imageID'] = 'wrong-' + config
        with self.assertRaises(RuntimeError):
            runner.select_ready_api_pod({'items': [pod]}, 'cma-matrix-old', config)


if __name__ == '__main__':
    unittest.main()
