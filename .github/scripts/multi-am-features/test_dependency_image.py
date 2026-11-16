import json
import unittest

from artifacts import canonical_reference, load_pinned_dependency


PIN = 'docker.io/library/mongo@sha256:' + '1' * 64
CONFIG = 'sha256:' + '2' * 64
MANIFEST = 'sha256:' + '3' * 64


class DependencyImageTests(unittest.TestCase):
    def test_docker_hub_default_names_preserve_registry_repository_and_digest(self):
        self.assertEqual(canonical_reference('mongo@' + PIN.split('@')[1]), PIN)
        self.assertEqual(canonical_reference('docker.io/mongo@' + PIN.split('@')[1]), PIN)
        for foreign in ['mirror.example/library/mongo', 'localhost/mongo', 'another/mongo',
                        'evil.example/docker.io/library/mongo', 'docker.io.evil.example/library/mongo']:
            self.assertNotEqual(canonical_reference(foreign + '@' + PIN.split('@')[1]), PIN)
        for invalid in ['https://docker.io/library/mongo', '/mongo', 'docker.io/../mongo']:
            with self.assertRaisesRegex(ValueError, 'pinned_dependency_required'):
                canonical_reference(invalid + '@' + PIN.split('@')[1])

    def fixture(self, *, foreign_config=False, missing_pin=False, architecture='amd64'):
        calls = []
        tag = 'localhost/multi-am-mongo:' + '1' * 64
        reference = 'localhost/multi-am-mongo@' + MANIFEST

        def command(argv, **kwargs):
            calls.append((argv, kwargs))
            if argv[:3] == ['docker', 'image', 'inspect']:
                return json.dumps([{'Id': CONFIG, 'Architecture': architecture, 'Os': 'linux',
                    'RepoDigests': [] if missing_pin else [PIN]}])
            if argv[-2:] == ['images', 'list']:
                return 'REF TYPE DIGEST SIZE PLATFORMS LABELS\n' + tag + ' type ' + MANIFEST + ' size linux/amd64 -\n'
            if 'inspecti' in argv:
                if argv[-1] != reference:
                    raise RuntimeError('registry_index_is_not_retained_by_docker_save')
                return json.dumps({'status': {'id': 'sha256:' + '4' * 64 if foreign_config else CONFIG}})
            return ''

        return calls, command, reference

    def test_registry_index_is_rebound_to_imported_manifest_with_same_config(self):
        calls, command, reference = self.fixture()
        result = load_pinned_dependency(command, 'owned-node', 'owned-cluster', 'mongo', PIN)
        self.assertEqual(result, reference)
        self.assertIn((['docker', 'pull', '--platform', 'linux/amd64', PIN], {'timeout': 180}), calls)
        self.assertTrue(any(argv[:3] == ['kind', 'load', 'docker-image'] for argv, _ in calls))
        self.assertFalse(any('inspecti' in argv and argv[-1] == PIN for argv, _ in calls))

    def test_changed_config_after_import_is_refused(self):
        _, command, _ = self.fixture(foreign_config=True)
        with self.assertRaisesRegex(ValueError, 'imported_config_mismatch'):
            load_pinned_dependency(command, 'owned-node', 'owned-cluster', 'mongo', PIN)

    def test_unbound_registry_digest_or_wrong_platform_never_loads_into_kind(self):
        for options, error in [({'missing_pin': True}, 'dependency_registry_pin_mismatch'),
                               ({'architecture': 'arm64'}, 'dependency_image_platform_mismatch')]:
            calls, command, _ = self.fixture(**options)
            with self.assertRaisesRegex(ValueError, error):
                load_pinned_dependency(command, 'owned-node', 'owned-cluster', 'mongo', PIN)
            self.assertFalse(any(argv[0] == 'kind' for argv, _ in calls))
