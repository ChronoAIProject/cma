from copy import deepcopy
import json
import unittest

from storage_fixture import bind_storage, storage_objects


class NativeStorageTests(unittest.TestCase):
    def manifest(self):
        return next(obj for obj in storage_objects() if obj['kind'] == 'StatefulSet')

    def command(self, architecture='amd64'):
        calls = []
        config = 'sha256:' + '1' * 64
        digest = 'sha256:' + '2' * 64
        tags = []

        def execute(argv, **_):
            calls.append(argv)
            if argv[:3] == ['docker', 'image', 'inspect']:
                return json.dumps([{'Id': config, 'Architecture': architecture,
                                    'Os': 'linux', 'RepoDigests': [argv[-1]]}])
            if argv[:3] == ['docker', 'image', 'tag']:
                tags.append(argv[-1])
            if argv[-2:] == ['images', 'list']:
                return '\n'.join(tag + ' type ' + digest + ' size linux/amd64 -' for tag in tags)
            if 'inspecti' in argv:
                return json.dumps({'status': {'id': config}})
            return ''
        return calls, execute

    def test_every_csi_container_uses_verified_import_and_preserves_storage_contract(self):
        original = self.manifest()
        calls, command = self.command()
        bound = bind_storage(original, command, 'owned-node', 'owned-cluster')
        spec = bound['spec']['template']['spec']
        self.assertEqual(spec['nodeSelector']['kubernetes.io/hostname'], 'owned-node')
        self.assertEqual(len([c for c in calls if c[:2] == ['docker', 'pull']]), 4)
        self.assertEqual(len([c for c in calls if 'inspecti' in c]), 4)
        for before, after in zip(original['spec']['template']['spec']['containers'], spec['containers']):
            self.assertTrue(after['image'].startswith('localhost/multi-am-storage-'))
            self.assertIn('@sha256:', after['image'])
            self.assertEqual(after['imagePullPolicy'], 'Never')
            restored = deepcopy(after)
            restored['image'] = before['image']
            restored['imagePullPolicy'] = before['imagePullPolicy']
            self.assertEqual(restored, before)
        self.assertEqual(spec['volumes'], original['spec']['template']['spec']['volumes'])
        self.assertEqual(original, self.manifest())

    def test_wrong_architecture_cannot_produce_deployable_storage_manifest(self):
        calls, command = self.command('arm64')
        with self.assertRaisesRegex(ValueError, 'dependency_image_platform_mismatch'):
            bind_storage(self.manifest(), command, 'owned-node', 'owned-cluster')
        self.assertFalse(any(c[0] in ('kind', 'kubectl') for c in calls))

    def test_extra_container_or_foreign_controller_refuses_before_import(self):
        for mutation in ['namespace', 'container', 'init']:
            manifest = self.manifest()
            spec = manifest['spec']['template']['spec']
            if mutation == 'namespace':
                manifest['metadata']['namespace'] = 'foreign'
            elif mutation == 'container':
                spec['containers'].append(deepcopy(spec['containers'][0]))
            else:
                spec['initContainers'] = [{'name': 'extra'}]
            calls, command = self.command()
            with self.assertRaises(ValueError):
                bind_storage(manifest, command, 'owned-node', 'owned-cluster')
            self.assertEqual(calls, [])
