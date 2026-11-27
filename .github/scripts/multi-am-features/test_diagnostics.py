import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import Mock, patch

from diagnostics import command_outcome, initialization_timeout, suite_error_frame, suite_error_kind
from infrastructure import Infrastructure


class DiagnosticTests(unittest.TestCase):
    def test_initialization_timeout_keeps_only_fixed_source_labels(self):
        self.assertEqual(initialization_timeout(TimeoutError('am_pod_not_ready_a')),
                         'am_pod_not_ready_a')
        for error in [TimeoutError('https://private.invalid?token=private'),
                      TimeoutError('am_pod_not_ready_private_user'),
                      RuntimeError('am_pod_not_ready_a')]:
            self.assertIsNone(initialization_timeout(error))

    def test_suite_output_keeps_only_the_javascript_error_class(self):
        self.assertEqual(suite_error_kind('first line\nTypeError: fixture-private\n    at /private/path'),
                         'TypeError')
        self.assertEqual(suite_error_kind('PrivateError: unreviewed'), 'unclassified')

    def test_missing_suite_coverage_reports_only_checked_out_source_location(self):
        root = Path(__file__).resolve().parent
        known = (root / 'features' / 'sharing.mjs').as_uri()
        unknown = (root / 'fixture_secret.mjs').as_uri()
        stack = (f'TypeError: fixture-private\n    at file:///outside/private.mjs:2:1\n'
                 f'    at {unknown}:4:5\n    at work ({known}:27:12)\n')
        self.assertEqual(suite_error_frame(stack, root), {'module': 'features/sharing.mjs', 'line': 27})
        self.assertEqual(suite_error_frame(f'{known}:1\nSyntaxError: fixture-private', root),
                         {'module': 'features/sharing.mjs', 'line': 1})
        self.assertIsNone(suite_error_frame(f'TypeError: {known}:3:1\n    at {unknown}:3:1', root))

    def test_suite_frame_handles_node_async_system_errors_and_numbered_source_directory(self):
        root = Path(__file__).resolve().parent
        suite = (root / 'suite.mjs').as_uri()
        binding = (root / 'sec01' / 'binding.mjs').as_uri()
        system_error = (f'Error: ENOENT: no such file or directory, open fixture-private\n'
                        f'    at async open (node:internal/fs/promises:637:25)\n'
                        f'    at async {suite}:49:29 {{\n  errno: -2,\n}}\n')
        self.assertEqual(suite_error_frame(system_error, root), {'module': 'suite.mjs', 'line': 49})
        self.assertEqual(suite_error_frame(f'    at bind ({binding}:31:11)\n', root),
                         {'module': 'sec01/binding.mjs', 'line': 31})

    def test_suite_frame_skips_installed_dependencies_and_symlinks(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            (root / 'suite.mjs').touch()
            dependency = root / 'node_modules' / 'ws' / 'wrapper.mjs'
            dependency.parent.mkdir(parents=True)
            dependency.touch()
            (root / 'alias.mjs').symlink_to(dependency)
            unavailable = (f'    at work ({dependency.as_uri()}:27:12)\n'
                           f'    at work ({(root / "alias.mjs").as_uri()}:7:3)\n')
            suite = f'    at work ({(root / "suite.mjs").as_uri()}:9:3)\n'
            self.assertIsNone(suite_error_frame(unavailable, root))
            self.assertEqual(suite_error_frame(unavailable + suite, root), {'module': 'suite.mjs', 'line': 9})

    def test_output_is_only_hashed_with_fixed_classification(self):
        private = b'private-token https://private.invalid?password=private-value\nno space left on device'
        result = command_outcome('image_api', 'docker', 1, 0.25, private)
        self.assertEqual(result['signals'], ['disk_full'])
        self.assertEqual(result['output_sha256'], hashlib.sha256(private).hexdigest())
        self.assertNotIn('private', json.dumps(result))
        self.assertNotIn('output', result)
        self.assertEqual(result['elapsed_ms'], 250)

    def test_startup_failure_markers_do_not_export_configuration_or_driver_details(self):
        private = b'configuration is not usable: private value\nCMA stopped private credentials'
        value = command_outcome('fixture', 'other', None, 0, private)
        self.assertEqual(value['signals'], ['configuration_refused', 'application_stopped'])
        self.assertNotIn('private', json.dumps(value))

    def test_failed_subprocess_records_first_failure_and_private_log_without_argv(self):
        with tempfile.TemporaryDirectory() as directory:
            infra = Infrastructure(directory)
            infra.stage = 'fixture_contract'
            with self.assertRaisesRegex(RuntimeError, 'fixture_command_failed'):
                infra.command([sys.executable, '-c',
                    "import sys;print('private-fixture-text resource mapping not found');sys.exit(3)"])
            first = infra.failed_command.copy()
            self.assertEqual(first['returncode'], 3)
            self.assertEqual(first['signals'], ['resource_mapping_missing'])
            self.assertEqual(first['program'], 'other')
            self.assertNotIn('private-fixture-text', json.dumps(infra.command_outcomes))
            self.assertIn('private-fixture-text', next(Path(directory).glob('command-*')).read_text())
            infra.stage = 'owned_cluster_cleanup'
            infra.command([sys.executable, '-c', 'print(1)'])
            self.assertEqual(infra.failed_command, first)
            self.assertTrue(all(p.poll() is not None for p in infra.children))

    def test_only_exact_cri_absence_can_be_a_pending_image_read(self):
        reference = 'localhost/runtime@sha256:' + 'a' * 64
        argv = ['docker', 'exec', 'fixture-node', 'crictl', 'inspecti', reference]
        pending = f'no such image "{reference}" present'
        with tempfile.TemporaryDirectory() as directory:
            infra = Infrastructure(directory)
            infra.stage = 'image_runtime_omarchy'

            def execute(output, *, allow):
                def spawn(_, *, stdout, **_kwargs):
                    process = Mock(returncode=1)
                    process.communicate.side_effect = lambda *_args, **_kwargs: stdout.write(output)
                    return process

                with patch('infrastructure.subprocess.Popen', side_effect=spawn):
                    return infra.command(argv, allow_pending_image=allow)

            for output in (pending, 'level=fatal msg=' + json.dumps(pending)):
                self.assertIsNone(execute(output, allow=True))
                self.assertEqual(infra.command_outcomes[-1]['signals'], ['cri_image_absent'])
                self.assertEqual(infra.command_outcomes[-1]['operation'], 'cri_image_status')
                self.assertIsNone(infra.failed_command)
            with self.assertRaisesRegex(RuntimeError, 'fixture_command_failed'):
                execute('no such image "unrelated" present', allow=True)
            self.assertEqual(infra.failed_command['signals'], [])
            with self.assertRaisesRegex(RuntimeError, 'fixture_command_failed'):
                execute(pending, allow=False)
            self.assertNotIn(reference, json.dumps(infra.command_outcomes))

    def test_timeout_joins_child_and_retains_original_stage(self):
        with tempfile.TemporaryDirectory() as directory:
            infra = Infrastructure(directory)
            infra.stage = 'kind_cluster_create'
            with self.assertRaises(subprocess.TimeoutExpired):
                infra.command([sys.executable, '-c', 'import time;time.sleep(10)'], timeout=0.05)
            self.assertTrue(infra.failed_command['timed_out'])
            self.assertEqual(infra.failed_command['stage'], 'kind_cluster_create')
            self.assertTrue(all(p.poll() is not None for p in infra.children))
