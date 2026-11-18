import copy
from contextlib import redirect_stdout
import hashlib
import io
import json
from pathlib import Path
import tempfile
import subprocess
import unittest
from unittest.mock import Mock, patch

import image_transfer as transfer


class TransferTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.env = dict(IMAGE_COMPONENT='web', IMAGE_NAME='cma-web', GITHUB_SHA='a' * 40,
                        GITHUB_REPOSITORY='ChronoAIProject/cma', GITHUB_RUN_ID='12',
                        GITHUB_RUN_ATTEMPT='2', GITHUB_EVENT_NAME='pull_request')
        self.archive = self.root / 'cma-web-linux-amd64.docker.tar.gz'
        self.archive.write_bytes(b'synthetic image archive')
        self.metadata = dict(schema='cma-ci-build-image-v1', component='web',
                             sourceRevision=self.env['GITHUB_SHA'], testedReference='cma-web:ci',
                             platform='linux/amd64', imageId='sha256:' + 'b' * 64,
                             archive=self.archive.name, archiveBytes=self.archive.stat().st_size,
                             archiveSha256=hashlib.sha256(self.archive.read_bytes()).hexdigest(),
                             workflow=dict(repository=self.env['GITHUB_REPOSITORY'], runId='12',
                                           runAttempt='2', event='pull_request'))
        self.persist()
        self.image = dict(Id=self.metadata['imageId'], Os='linux', Architecture='amd64',
                          Config={'Labels': {'org.opencontainers.image.revision': self.env['GITHUB_SHA']}})

    def persist(self):
        (self.root / 'metadata.json').write_text(json.dumps(self.metadata))

    def test_verified_immutable_image_is_loaded_then_tagged_without_build(self):
        with patch.object(transfer.subprocess, 'run') as run, patch.object(
                transfer.subprocess, 'check_output', return_value=json.dumps([self.image])):
            transfer.load(self.root, self.env)
        self.assertEqual(run.call_args_list[0].args[0][:3], ['docker', 'image', 'load'])
        self.assertEqual(run.call_args_list[1].args[0],
                         ['docker', 'image', 'tag', self.metadata['imageId'], 'cma-web:ci'])

    def test_unqualified_source_component_reference_and_run_are_rejected(self):
        for key, value in [('schema', 'cma-ci-native-image-v1'), ('component', 'api'),
                           ('sourceRevision', 'c' * 40), ('testedReference', 'other:ci'),
                           ('platform', 'linux/arm64'), ('archive', '../outside'),
                           ('archiveBytes', 0), ('imageId', 'mutable:tag')]:
            changed = dict(self.metadata, **{key: value})
            with self.subTest(key=key), self.assertRaises(ValueError):
                transfer.validate(changed, self.root, self.env)
        for key in self.metadata['workflow']:
            changed = copy.deepcopy(self.metadata)
            changed['workflow'][key] = 'other'
            with self.subTest(key=key), self.assertRaises(ValueError):
                transfer.validate(changed, self.root, self.env)

    def test_corrupt_or_symlink_archive_is_rejected_before_docker(self):
        self.archive.write_bytes(b'changed image archive')
        with patch.object(transfer.subprocess, 'run') as run, self.assertRaises(ValueError):
            transfer.load(self.root, self.env)
        run.assert_not_called()
        self.archive.unlink()
        self.archive.symlink_to(self.root / 'metadata.json')
        with self.assertRaises(ValueError):
            transfer.validate(self.metadata, self.root, self.env)

    def test_imported_image_must_match_before_tagging(self):
        for key, value in [('Id', 'sha256:' + 'c' * 64), ('Os', 'windows'), ('Architecture', 'arm64'),
                           ('Config', {'Labels': {'org.opencontainers.image.revision': 'd' * 40}})]:
            with self.subTest(key=key), patch.object(transfer.subprocess, 'run') as run, patch.object(
                    transfer.subprocess, 'check_output', return_value=json.dumps([dict(self.image, **{key: value})])):
                with self.assertRaises(ValueError):
                    transfer.load(self.root, self.env)
                self.assertEqual(run.call_count, 1)

    def test_save_reuses_exporter_but_marks_artifact_unqualified(self):
        legacy = dict(self.metadata, schema='cma-ci-native-image-v1')
        def export(*_):
            (self.root / 'metadata.json').write_text(json.dumps(legacy))
        module = Mock()
        module.export.side_effect = export
        with patch.object(transfer, 'exporter', return_value=module):
            transfer.save(self.root, self.env)
        module.export.assert_called_once_with('web', self.env['GITHUB_SHA'], self.root)
        self.assertEqual(json.loads((self.root / 'metadata.json').read_text())['schema'], 'cma-ci-build-image-v1')

    def test_exporter_is_the_existing_native_image_export_implementation(self):
        module = transfer.exporter()
        self.assertTrue(callable(module.export))
        self.assertTrue(module.__file__.endswith('/export-ci-native-image.py'))

    def test_cli_preserves_success_and_reports_errors_without_payload(self):
        for command in ('save', 'load'):
            with patch.dict(transfer.os.environ, dict(self.env, RUNNER_TEMP=str(self.root))), patch(
                    'sys.argv', ['image_transfer', command]), patch.object(transfer, command) as operation:
                with redirect_stdout(io.StringIO()):
                    self.assertEqual(transfer.main(), 0)
                operation.assert_called_once()
                operation.side_effect = subprocess.CalledProcessError(1, ['docker'], stderr='private payload')
                output = io.StringIO()
                with redirect_stdout(output):
                    self.assertEqual(transfer.main(), 1)
                self.assertNotIn('private payload', output.getvalue())
        with patch('sys.argv', ['image_transfer', 'unknown']), redirect_stdout(io.StringIO()):
            self.assertEqual(transfer.main(), 1)


if __name__ == '__main__':
    unittest.main()
