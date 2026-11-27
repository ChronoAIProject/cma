import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from diagnostics import command_outcome, initialization_timeout
from infrastructure import Infrastructure


class DiagnosticTests(unittest.TestCase):
    def test_initialization_timeout_keeps_only_fixed_source_labels(self):
        self.assertEqual(initialization_timeout(TimeoutError('am_pod_not_ready_a')),
                         'am_pod_not_ready_a')
        for error in [TimeoutError('https://private.invalid?token=private'),
                      TimeoutError('am_pod_not_ready_private_user'),
                      RuntimeError('am_pod_not_ready_a')]:
            self.assertIsNone(initialization_timeout(error))

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

    def test_timeout_joins_child_and_retains_original_stage(self):
        with tempfile.TemporaryDirectory() as directory:
            infra = Infrastructure(directory)
            infra.stage = 'kind_cluster_create'
            with self.assertRaises(subprocess.TimeoutExpired):
                infra.command([sys.executable, '-c', 'import time;time.sleep(10)'], timeout=0.05)
            self.assertTrue(infra.failed_command['timed_out'])
            self.assertEqual(infra.failed_command['stage'], 'kind_cluster_create')
            self.assertTrue(all(p.poll() is not None for p in infra.children))

    def test_read_retry_retains_attempts_and_reports_only_terminal_failure(self):
        with tempfile.TemporaryDirectory() as directory, patch('infrastructure.time.sleep'):
            infra = Infrastructure(directory)
            infra.stage = 'image_runtime_omarchy'
            marker = Path(directory, 'read-attempt')
            recovering = [sys.executable, '-c',
                "from pathlib import Path; import sys; p = Path(sys.argv[1]); "
                "found = p.exists(); p.touch(); sys.exit(0 if found else 3)", str(marker)]
            self.assertEqual(infra.command(recovering, attempts=3), '')
            self.assertEqual([row['returncode'] for row in infra.command_outcomes], [3, 0])
            self.assertIsNone(infra.failed_command)
            with self.assertRaisesRegex(RuntimeError, 'fixture_command_failed'):
                infra.command([sys.executable, '-c', 'raise SystemExit(4)'], attempts=2)
            self.assertEqual([row['returncode'] for row in infra.command_outcomes], [3, 0, 4, 4])
            self.assertEqual(infra.failed_command['returncode'], 4)
            self.assertTrue(all(p.poll() is not None for p in infra.children))
