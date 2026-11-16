#!/usr/bin/env python3
import base64
import copy
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
        'Config': {'Labels': {'org.opencontainers.image.revision': revision,
                              'org.opencontainers.image.source': 'https://github.com/ChronoAIProject/cma'}},
    }]


class MigrationKindRunnerTests(unittest.TestCase):
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

    def test_mongo_init_authenticates_same_database_and_waits_for_primary(self):
        script = runner.mongo_documents({'createRole': 'cma_am_runtime', 'roles': [], 'privileges': []})
        self.assertLess(script.index('rs.initiate'), script.index('createRole'))
        self.assertLess(script.index('rs.isMaster().ismaster'), script.index('createUser'))
        self.assertIn('pwd: process.env.MATRIX_RUNTIME_PASSWORD', script)
        self.assertIn('--authenticationDatabase matrix', script)

    def test_local_manifest_id_is_not_mislabeled_as_config_digest(self):
        value = runner.checked_identity(lambda argv: json.dumps(inspect()), IMAGE, OLD)
        self.assertNotIn('config', value)
        self.assertEqual(value['local_image_id'], inspect()[0]['Id'])

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
            campaign = runner.Campaign(Path(directory), '/bin/false', 1, plan)
            self.assertEqual(campaign.cluster.startswith('cma-matrix-'), True)
            self.assertTrue(campaign.kubeconfig.name.endswith('.private'))

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


if __name__ == '__main__':
    unittest.main()
