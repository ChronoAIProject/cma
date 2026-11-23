import hashlib
import json
import os
from pathlib import Path
import secrets
import sys
import tempfile
import unittest
from unittest.mock import patch

from migration_native_process import Command, Forward, Journal, NativeProcessError, environment, private_directory


class ProcessTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.directory = Path(self.temp.name).resolve()
        self.events = []

    def test_actual_private_stdin_binary_stdout_and_no_credential_logging(self):
        secret = secrets.token_bytes(32)
        command = Command(self.events.append)
        result = command([sys.executable, '-c', 'import sys;sys.stdout.buffer.write(sys.stdin.buffer.read())'],
                         input_text=secret)
        self.assertEqual(result, secret)
        self.assertNotIn(secret.hex(), str(self.events))
        self.assertEqual(self.events[-1]['stdout_sha256'], hashlib.sha256(secret).hexdigest())
        self.assertTrue(self.events[-1]['processes']['process_group_cleanup'])

    def test_nonzero_timeout_output_bounds_and_start_failure_are_joined(self):
        for argv, options in [
            ([sys.executable, '-c', 'raise SystemExit(4)'], {}),
            ([sys.executable, '-c', 'import time;time.sleep(10)'], {'timeout': .05}),
            ([sys.executable, '-c', 'print("x"*10000)'], {'max_bytes': 10}),
            (['/nonexistent-native-test-binary'], {}),
        ]:
            with self.subTest(argv=argv):
                with self.assertRaises(NativeProcessError): Command(self.events.append)(argv, **options)
                self.assertEqual(self.events[-1]['outcome'], 'FAILED')
                self.assertTrue(self.events[-1]['processes']['process_group_cleanup'])

    def test_command_tick_failure_and_interrupt_clean_up(self):
        def fail(): raise RuntimeError('private tick failed')
        with self.assertRaisesRegex(NativeProcessError, '^native_command_failed$'):
            Command(self.events.append, tick=fail)([sys.executable, '-c', 'import time;time.sleep(10)'])
        self.assertTrue(self.events[-1]['processes']['process_group_cleanup'])
        def interrupt(): raise KeyboardInterrupt()
        with self.assertRaises(KeyboardInterrupt):
            Command(self.events.append, tick=interrupt)([sys.executable, '-c', 'import time;time.sleep(10)'])
        self.assertTrue(self.events[-1]['processes']['process_group_cleanup'])

    def test_large_stdin_is_drained_concurrently_and_no_inherited_cloud_environment(self):
        data = b'x' * (1024**2)
        command = Command(self.events.append)
        result = command([sys.executable, '-c',
            'import sys;sys.stderr.write("header");sys.stdout.buffer.write(sys.stdin.buffer.read())'], input_text=data)
        self.assertEqual(result, data)
        with patch.dict(os.environ, {'AWS_SECRET_ACCESS_KEY': secrets.token_hex(24), 'KUBECONFIG': 'foreign',
                                     'DOCKER_HOST': 'tcp://foreign', 'DOCKER_CONTEXT': 'foreign'}):
            self.assertFalse(set(environment()) & {'AWS_SECRET_ACCESS_KEY', 'KUBECONFIG', 'DOCKER_HOST', 'DOCKER_CONTEXT'})

    def test_input_validation_and_finality_failure(self):
        for argv, options in [(None, {}), ([], {}), (['bad\0arg'], {}),
                              ([sys.executable], {'input_text': object()}),
                              ([sys.executable], {'max_bytes': 0})]:
            with self.assertRaises(NativeProcessError): Command(self.events.append)(argv, **options)
        self.assertEqual(self.events, [])

    def test_forward_real_process_bind_and_idempotent_cleanup(self):
        code = 'import time;print("Forwarding from 127.0.0.1:43210 -> 8080",flush=True);time.sleep(10)'
        forward = Forward([sys.executable, '-c', code], 8080, self.events.append)
        self.addCleanup(forward.close)
        forward.start();self.assertEqual(forward.port, 43210);forward.pump()
        with self.assertRaises(NativeProcessError): forward.start()
        receipt = forward.close()
        self.assertTrue(receipt['processes']['process_group_cleanup'])
        self.assertIs(forward.close(), receipt)
        forward.pump()

    def test_invalid_and_dead_forwards_never_bind(self):
        for line in ('Forwarding from 0.0.0.0:43210 -> 8080', 'Forwarding from 127.0.0.1:43210 -> 8081',
                     'Forwarding from 127.0.0.1:99999 -> 8080', 'x'*5000):
            code = 'import time;print('+repr(line)+',flush=True);time.sleep(10)'
            forward = Forward([sys.executable, '-c', code], 8080, self.events.append)
            with self.assertRaises(NativeProcessError): forward.start()
            self.assertTrue(forward.close()['processes']['process_group_cleanup'])
        forward = Forward([sys.executable, '-c', 'raise SystemExit(1)'], 8080, self.events.append)
        with self.assertRaises(NativeProcessError): forward.start()
        self.assertTrue(forward.close()['processes']['process_group_cleanup'])
        with self.assertRaises(NativeProcessError): Forward([], 0, self.events.append)

    def test_journal_hash_chain_private_modes_redaction_and_poisoned_write(self):
        secret = secrets.token_urlsafe(24)
        journal = Journal(self.directory);journal.redactor.remember(secret)
        journal.record({'bearer': secret});journal.record({'stage': 'old-write'})
        receipt = journal.close()
        self.assertFalse(receipt['poisoned']);self.assertEqual(receipt['records'], 2)
        self.assertEqual(journal.path.stat().st_mode & 0o777, 0o600)
        data = journal.path.read_text();self.assertNotIn(secret, data)
        previous = None
        for row in map(json.loads, data.splitlines()):
            checksum = row.pop('sha256')
            self.assertEqual(row['previous'], previous)
            self.assertEqual(checksum, hashlib.sha256(json.dumps(row, sort_keys=True, separators=(',', ':')).encode()).hexdigest())
            previous = checksum
        with self.assertRaises(NativeProcessError): journal.record({})
        with self.assertRaises(FileExistsError): Journal(self.directory)
        bad = Journal(self.directory, 'failure.jsonl')
        with patch('migration_native_process.os.fsync', side_effect=OSError('disk full')):
            with self.assertRaises(OSError): bad.record({'write': 1})
        with self.assertRaises(NativeProcessError): bad.record({'write': 2})
        self.assertTrue(bad.close()['poisoned'])

    def test_private_directory_and_journal_bounds(self):
        alias = self.directory/'link';alias.symlink_to(self.directory, target_is_directory=True)
        with self.assertRaises(NativeProcessError): private_directory(alias)
        with self.assertRaises(NativeProcessError): Journal(self.directory, '../escape')
        value = Journal(self.directory, 'bound.jsonl')
        with self.assertRaises(NativeProcessError): value.record({'value': 'x'*(4*1024**2)})
        self.assertTrue(value.close()['poisoned'])
        value = Journal(self.directory, 'total.jsonl');value.bytes = 64*1024**2
        with self.assertRaises(NativeProcessError): value.record({'value': 1})
        value.close()


if __name__ == '__main__':
    unittest.main()
