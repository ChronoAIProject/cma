"""Image checks must remain auditable even when no cluster is created."""
import json
from pathlib import Path
import subprocess
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

import migration_kind_runner as runner
from test_migration_kind_runner import IMAGE, OLD, inspect


class ImagePreflightTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.output = self.root / 'run'
        self.args = SimpleNamespace(output=self.output, old_image=IMAGE,
                                    new_image=IMAGE, old_source=OLD, new_source=OLD,
                                    plan=self.root / 'missing-plan.json')

    def observations(self):
        path = self.output / 'image-preflight.private.json'
        self.assertEqual(path.stat().st_mode & 0o777, 0o600)
        document = json.loads(path.read_text())
        self.assertEqual(document['schema'], 'migration-image-preflight-v1')
        return document['observations']

    def run_to_missing_plan(self, value):
        with patch.object(runner.subprocess, 'check_output', return_value=value), \
                patch.object(runner, 'Campaign') as campaign:
            with self.assertRaisesRegex(runner.RunnerError, 'plan is missing'):
                runner.run(self.args)
            campaign.assert_not_called()

    def test_valid_images_are_recorded_before_any_cluster_or_plan_load(self):
        value = inspect()
        value[0]['Config']['Env'] = ['DO_NOT_RECORD=private-image-value']
        value[0]['Config']['Labels']['unrelated'] = 'private-label-value'
        value[0]['ContainerConfig'] = {'Cmd': ['private-build-value']}
        self.run_to_missing_plan(json.dumps(value))
        rows = self.observations()
        self.assertEqual(len(rows), 2)
        for row in rows:
            self.assertEqual(row['argv'], ['docker', 'image', 'inspect', IMAGE])
            self.assertEqual(row['exit'], 0)
            self.assertLessEqual(row['started_at'], row['finished_at'])
            self.assertEqual(row['metadata'], inspect())
            self.assertEqual(runner.checked_identity(
                lambda argv: json.dumps(row['metadata']), IMAGE, OLD)['source'], OLD)
        self.assertNotIn('private-', json.dumps(rows))

    def test_wrong_source_is_preserved_and_blocks_cluster_creation(self):
        wrong = inspect(revision='b' * 40)
        with patch.object(runner.subprocess, 'check_output', return_value=json.dumps(wrong)), \
                patch.object(runner, 'Campaign') as campaign:
            with self.assertRaisesRegex(runner.RunnerError, 'source label'):
                runner.run(self.args)
            campaign.assert_not_called()
        rows = self.observations()
        self.assertEqual(len(rows), 1)
        self.assertEqual(rows[0]['metadata'], wrong)

    def test_transport_failure_keeps_error_without_private_output(self):
        failure = subprocess.CalledProcessError(7, ['docker'],
                                                output='private-output', stderr='private-error')
        with patch.object(runner.subprocess, 'check_output', side_effect=failure):
            with self.assertRaisesRegex(runner.RunnerError, 'not available locally'):
                runner.run(self.args)
        row = self.observations()[0]
        self.assertEqual(row['exit'], 7)
        self.assertEqual(row['error'], 'CalledProcessError')
        self.assertNotIn('private-', json.dumps(row))

    def test_malformed_inspection_fails_without_saving_untrusted_output(self):
        for value in ('private-invalid-json', '{}', '[null]'):
            with self.subTest(value=value), tempfile.TemporaryDirectory() as directory:
                self.output = Path(directory) / 'run'
                self.args.output = self.output
                with patch.object(runner.subprocess, 'check_output', return_value=value):
                    with self.assertRaises((runner.RunnerError, ValueError)):
                        runner.run(self.args)
                row = self.observations()[0]
                self.assertEqual(row['error'], 'invalid_image_metadata')
                self.assertNotIn('private-', json.dumps(row))

    def test_partial_success_survives_later_image_failure(self):
        failure = subprocess.CalledProcessError(1, ['docker'])
        with patch.object(runner.subprocess, 'check_output',
                          side_effect=[json.dumps(inspect()), failure]):
            with self.assertRaises(runner.RunnerError):
                runner.run(self.args)
        rows = self.observations()
        self.assertEqual([row['exit'] for row in rows], [0, 1])
        self.assertEqual(rows[0]['metadata'], inspect())

    def test_interruption_preserves_one_observation_and_does_not_retry(self):
        self.output.mkdir()
        command = Mock(side_effect=KeyboardInterrupt())
        recorder = runner.ImagePreflight(self.output, command)
        with self.assertRaises(KeyboardInterrupt):
            recorder(['docker', 'image', 'inspect', IMAGE])
        command.assert_called_once()
        row = self.observations()[0]
        self.assertIsNone(row['exit'])
        self.assertEqual(row['error'], 'KeyboardInterrupt')

    def test_recorder_rejects_non_inspection_commands_before_invocation(self):
        self.output.mkdir()
        command = Mock()
        recorder = runner.ImagePreflight(self.output, command)
        with self.assertRaisesRegex(ValueError, 'only permits'):
            recorder(['docker', 'run', IMAGE])
        command.assert_not_called()
        self.assertEqual(list(self.output.iterdir()), [])

    def test_dependency_without_oci_labels_still_replays(self):
        self.output.mkdir()
        metadata = inspect()
        del metadata[0]['Config']
        del metadata[0]['Descriptor']
        recorder = runner.ImagePreflight(self.output, lambda argv: json.dumps(metadata))
        actual = runner.checked_dependency(recorder, IMAGE)
        saved = self.observations()[0]['metadata']
        self.assertEqual(actual, runner.checked_dependency(lambda argv: json.dumps(saved), IMAGE))

    def test_invalid_nested_metadata_is_retained_as_failure(self):
        for key, value in [('Config', {'Labels': ['invalid']}),
                           ('Descriptor', {'annotations': ['invalid']})]:
            with self.subTest(key=key), tempfile.TemporaryDirectory() as directory:
                self.output = Path(directory)
                metadata = inspect()
                metadata[0][key] = value
                recorder = runner.ImagePreflight(self.output, lambda argv: json.dumps(metadata))
                with self.assertRaisesRegex(ValueError, 'invalid image inspection'):
                    recorder(['docker', 'image', 'inspect', IMAGE])
                self.assertEqual(self.observations()[0]['error'], 'invalid_image_metadata')


if __name__ == '__main__':
    unittest.main()
