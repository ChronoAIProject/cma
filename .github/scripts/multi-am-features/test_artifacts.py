import gzip
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from artifacts import SOURCE, checked_metadata, import_tag, load_image

REVISION = 'a' * 40
TREE = 'b' * 40
IMAGE_ID = 'sha256:' + 'c' * 64
MANIFEST_ID = 'sha256:' + 'd' * 64
RUN_ID = '12'
ATTEMPT = '2'
COMPONENT = 'runtime'
ARCHIVE_NAME = 'cma-runtime-linux-amd64.docker.tar.gz'
IMAGE = {
    'Id': IMAGE_ID,
    'Os': 'linux',
    'Architecture': 'amd64',
    'Config': {'Labels': {
        'org.opencontainers.image.source': SOURCE,
        'org.opencontainers.image.revision': REVISION,
        'org.opencontainers.image.version': 'git-' + REVISION[:12],
        'dev.chrono.cma.runtime-contract': '3',
    }},
}


class ArtifactTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name)
        source = Path(__file__).resolve().parents[1] / 'export-ci-native-image.py'
        spec = importlib.util.spec_from_file_location('native_image_export', source)
        self.producer = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.producer)
        self.root = self.directory / 'artifact'
        self.export(self.root, 'schedule', 'refs/heads/develop')
        self.manifest = self.root / 'metadata.json'
        self.metadata = json.loads(self.manifest.read_text())

    def export(self, destination, event, ref, *, component=COMPONENT):
        producer = self.producer
        responses = [REVISION, TREE, json.dumps([IMAGE])]
        with patch.dict(producer.os.environ, {
                'GITHUB_REPOSITORY': 'ChronoAIProject/cma', 'GITHUB_EVENT_NAME': event,
                'GITHUB_RUN_ID': RUN_ID, 'GITHUB_RUN_ATTEMPT': ATTEMPT, 'GITHUB_REF': ref,
        }), patch.object(producer, 'output', side_effect=responses), patch.object(
                producer.subprocess, 'run'), patch.object(producer.subprocess, 'Popen') as save, patch(
                'sys.stdout', new_callable=io.StringIO):
            stream = save.return_value.__enter__.return_value
            stream.stdout = io.BytesIO(b'native image fixture')
            stream.wait.return_value = 0
            producer.export(component, REVISION, destination)
            save.assert_called_once_with(['docker', 'image', 'save', IMAGE_ID], stdout=subprocess.PIPE)

    def check_metadata(self):
        return checked_metadata(self.root, COMPONENT, REVISION, RUN_ID, ATTEMPT, source_tree=TREE)

    def test_archive_from_current_workflow_export_is_accepted(self):
        data, archive = self.check_metadata()
        self.assertEqual(data['imageId'], IMAGE_ID)
        self.assertEqual(data['workflow']['event'], 'schedule')
        self.assertEqual(archive.name, ARCHIVE_NAME)
        self.assertEqual(gzip.decompress(archive.read_bytes()), b'native image fixture')

    def test_manual_verification_only_exports_fixture_components_from_develop(self):
        self.export(self.directory / 'manual', 'workflow_dispatch', 'refs/heads/develop')
        for ref, component in [('refs/heads/feature', COMPONENT), ('refs/heads/develop', 'web')]:
            with self.subTest(ref=ref, component=component):
                target = self.directory / 'refused'
                with self.assertRaisesRegex(ValueError, 'CI event identity required'):
                    self.export(target, 'workflow_dispatch', ref, component=component)
                self.assertFalse(target.exists())

    def test_existing_merge_group_export_still_records_the_event(self):
        target = self.directory / 'merge-group'
        self.export(target, 'merge_group', 'refs/heads/queue-example')
        data, _ = checked_metadata(target, COMPONENT, REVISION, RUN_ID, ATTEMPT, source_tree=TREE)
        self.assertEqual(data['workflow']['event'], 'merge_group')

    def test_wrong_revision_tree_run_and_attempt_each_fail(self):
        for key, value, error in [
            ('sourceRevision', 'e' * 40, 'identity_mismatch'),
            ('sourceTree', 'e' * 40, 'identity_mismatch'),
            ('workflow', {**self.metadata['workflow'], 'runId': '13'}, 'run_mismatch'),
            ('workflow', {**self.metadata['workflow'], 'runAttempt': '1'}, 'run_mismatch'),
        ]:
            with self.subTest(key=key, value=value):
                self.manifest.write_text(json.dumps({**self.metadata, key: value}))
                with self.assertRaisesRegex(ValueError, error):
                    self.check_metadata()

    def test_modified_archive_with_original_size_fails(self):
        archive = self.root / ARCHIVE_NAME
        data = archive.read_bytes()
        archive.write_bytes(bytes([data[0] ^ 1]) + data[1:])
        with self.assertRaisesRegex(ValueError, 'archive_hash'):
            self.check_metadata()

    def test_path_escape_and_symlink_fail(self):
        self.manifest.write_text(json.dumps({**self.metadata, 'archive': '../' + ARCHIVE_NAME}))
        with self.assertRaisesRegex(ValueError, 'archive_path'):
            self.check_metadata()
        self.manifest.write_text(json.dumps(self.metadata))
        archive = self.root / ARCHIVE_NAME
        outside = archive.rename(self.root.parent / ARCHIVE_NAME)
        archive.symlink_to(outside)
        with self.assertRaisesRegex(ValueError, 'archive_size'):
            self.check_metadata()

    def loader(self, image):
        calls = []
        tag = f'localhost/multi-am-{COMPONENT}:{REVISION}'
        reference = f'localhost/multi-am-{COMPONENT}@{MANIFEST_ID}'

        def command(argv, **_):
            calls.append(argv)
            if argv[:3] == ['docker', 'image', 'inspect']:
                return json.dumps([image])
            if argv[-2:] == ['images', 'list']:
                return f'{tag} type {MANIFEST_ID} size linux/amd64 -\n'
            if argv[-2:] == ['inspecti', reference]:
                return json.dumps({'status': {'id': IMAGE_ID}})
            return ''

        return calls, command, reference

    def load(self, command):
        return load_image(command, 'test-node', 'test-cluster', self.root, COMPONENT, REVISION,
                          RUN_ID, ATTEMPT, source_tree=TREE)

    def test_loaded_config_is_joined_to_the_imported_manifest(self):
        calls, command, reference = self.loader(IMAGE)
        self.assertEqual(self.load(command), reference)
        self.assertIn(['docker', 'exec', 'test-node', 'crictl', 'inspecti', reference], calls)

    def test_import_waits_only_for_an_absent_exact_cri_image(self):
        tag = f'localhost/multi-am-{COMPONENT}:{REVISION}'
        replies = iter((None, None, json.dumps({'status': {'id': IMAGE_ID}})))
        calls, base, reference = self.loader(IMAGE)
        inspect = ['docker', 'exec', 'test-node', 'crictl', 'inspecti', reference]
        attempts = []

        def command(argv, **kwargs):
            if argv == inspect:
                attempts.append(kwargs)
                return next(replies)
            return base(argv, **kwargs)

        with patch('artifacts.time.sleep') as sleep:
            self.assertEqual(import_tag(command, 'test-node', 'test-cluster', tag, IMAGE_ID), reference)
        self.assertEqual(attempts, [{'timeout': 10, 'allow_pending_image': True}] * 3)
        self.assertEqual([call.args for call in sleep.call_args_list], [(1,), (2,)])
        self.assertEqual(sum(argv[0] == 'kind' for argv in calls), 1)

    def test_missing_cri_image_does_not_bypass_the_final_read(self):
        attempts = []
        tag = f'localhost/multi-am-{COMPONENT}:{REVISION}'
        _, base, reference = self.loader(IMAGE)

        def command(argv, **kwargs):
            if argv[-2:] == ['inspecti', reference]:
                attempts.append(kwargs)
                if kwargs.get('allow_pending_image'):
                    return None
                raise RuntimeError('fixture_command_failed')
            return base(argv, **kwargs)

        with patch('artifacts.time.sleep'), self.assertRaisesRegex(RuntimeError, 'fixture_command_failed'):
            import_tag(command, 'test-node', 'test-cluster', tag, IMAGE_ID)
        self.assertEqual(attempts, [{'timeout': 10, 'allow_pending_image': True}] * 3 + [{'timeout': 10}])

    def test_mismatched_loaded_runtime_contract_fails_before_kind_import(self):
        labels = {**IMAGE['Config']['Labels'], 'dev.chrono.cma.runtime-contract': '2'}
        calls, command, _ = self.loader({**IMAGE, 'Config': {'Labels': labels}})
        with self.assertRaisesRegex(ValueError, 'loaded_image_identity_mismatch'):
            self.load(command)
        self.assertFalse(any(argv[0] == 'kind' for argv in calls))


if __name__ == '__main__':
    unittest.main()
