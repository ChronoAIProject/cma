"""Dependencies fail closed on drift before any isolated infrastructure runs."""
from copy import deepcopy
import json
from pathlib import Path
import shutil
import tempfile
import unittest

from migration_native_dependencies import (
    ASSETS, dependencies, object_key, owned_cluster, read_pinned, runtime_identity, unique_objects,
)
from migration_native_test_fixture import CLUSTER, NODE, runtime


class NativeDependencyTests(unittest.TestCase):
    def test_actual_pins_bind_real_crds_and_every_dependency(self):
        objects, contract = dependencies(CLUSTER, NODE)
        self.assertEqual(len(objects), 25)
        self.assertEqual(len(contract['images']), 5)
        self.assertEqual(contract['readiness_status'], 'NOT_PROVEN')
        self.assertTrue(contract['exclusive_disposable_cluster_required'])
        self.assertEqual({o['metadata']['name'] for o in objects if o['kind'] == 'CustomResourceDefinition'}, {
            'sandboxes.agents.x-k8s.io', 'sandboxclaims.extensions.agents.x-k8s.io',
            'sandboxtemplates.extensions.agents.x-k8s.io', 'sandboxwarmpools.extensions.agents.x-k8s.io'})
        for obj in objects:
            if obj['kind'] == 'CustomResourceDefinition':
                self.assertIn('spec', obj['spec']['versions'][0]['schema']['openAPIV3Schema']['properties'])
            if obj['kind'] in ('Deployment', 'StatefulSet'):
                spec = obj['spec']['template']['spec']
                self.assertEqual(spec['nodeSelector'], {'kubernetes.io/hostname': NODE})
                for container in spec['containers']:
                    self.assertEqual(container['imagePullPolicy'], 'Never')
                    self.assertEqual(container['image'], contract['images'][container['name']]['image'])
        self.assertEqual((objects, contract), dependencies(CLUSTER, NODE))

    def test_pinned_assets_cannot_be_changed_or_substituted(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            shutil.copytree(ASSETS, root / 'assets')
            root /= 'assets'
            filename = root / 'controller-01.json'
            filename.write_text('{}')
            with self.assertRaisesRegex(ValueError, 'hash_mismatch'):
                dependencies(CLUSTER, NODE, directory=root)
            filename.unlink()
            filename.symlink_to(ASSETS / filename.name)
            with self.assertRaisesRegex(ValueError, 'file_invalid'):
                dependencies(CLUSTER, NODE, directory=root)
            filename.unlink()
            with self.assertRaisesRegex(ValueError, 'file_invalid'):
                dependencies(CLUSTER, NODE, directory=root)
            (root / 'manifest.json').write_text('{}')
            with self.assertRaisesRegex(ValueError, 'hash_mismatch'):
                dependencies(CLUSTER, NODE, directory=root)
            huge = root / 'huge'
            huge.write_bytes(b' ' * (1024 * 1024 + 1))
            with self.assertRaisesRegex(ValueError, 'file_invalid'):
                read_pinned(root, 'huge', '0' * 64)

    def test_foreign_cluster_and_node_refused(self):
        for cluster in (None, '', 'production', 'cma-matrix-01234567890', 'cma-matrix-ABCDEF1234'):
            with self.subTest(cluster=cluster), self.assertRaisesRegex(ValueError, 'owned_cluster'):
                owned_cluster(cluster, NODE)
        with self.assertRaisesRegex(ValueError, 'owned_node'):
            owned_cluster(CLUSTER, 'other-control-plane')

    def test_duplicate_or_malformed_identity_refused(self):
        obj = {'apiVersion': 'v1', 'kind': 'Namespace', 'metadata': {'name': 'fixture'}}
        self.assertEqual(object_key(obj), ('v1', 'Namespace', '', 'fixture'))
        with self.assertRaisesRegex(ValueError, 'duplicate_object'):
            unique_objects([obj, deepcopy(obj)])
        for malformed in ({}, None, {'apiVersion': 'v1', 'kind': 'Pod', 'metadata': None}):
            with self.assertRaisesRegex(ValueError, 'object_identity'):
                object_key(malformed)

    def test_runtime_source_manifest_and_config_join(self):
        identity, manifest, config = runtime()
        self.assertEqual(runtime_identity(identity, manifest, config), identity)
        for field, value in [('source', 'main'), ('config', 'sha256:bad'),
                             ('image', 'runtime:latest'), ('image', 'registry/runtime@sha256:' + '0' * 64)]:
            invalid = {**identity, field: value}
            with self.subTest(field=field), self.assertRaises(ValueError):
                runtime_identity(invalid, manifest, config)
        for invalid in (None, {}, {**identity, 'unexpected': True}, {**identity, 'image': 42}):
            with self.assertRaisesRegex(ValueError, 'identity_invalid'):
                runtime_identity(invalid, manifest, config)
        for raw in ('not-bytes', b'x' * (1024 * 1024 + 1)):
            with self.assertRaisesRegex(ValueError, 'raw_metadata'):
                runtime_identity(identity, raw, config)
        with self.assertRaisesRegex(ValueError, 'digest_mismatch'):
            runtime_identity(identity, manifest, config + b' ')

    def test_architecture_source_and_descriptor_mismatch_refused(self):
        config = json.loads(runtime()[2])
        candidates = [None, {}, {**config, 'os': 'windows'}, {**config, 'architecture': 'arm64'},
                      {**config, 'config': {'Labels': {'org.opencontainers.image.revision': 'b' * 40}}}]
        for bad in candidates[1:]:
            with self.subTest(config=bad), self.assertRaisesRegex(ValueError, 'metadata_'):
                runtime_identity(*runtime(config=bad))
        for bad in ({}, {'schemaVersion': 1, 'config': {}},
                    {'schemaVersion': 2, 'config': {'digest': 'sha256:' + '0' * 64, 'size': 0}}):
            with self.subTest(manifest=bad), self.assertRaisesRegex(ValueError, 'metadata_'):
                runtime_identity(*runtime(manifest=bad))


if __name__ == '__main__':
    unittest.main()
