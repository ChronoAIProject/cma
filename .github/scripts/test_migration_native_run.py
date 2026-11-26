from contextlib import redirect_stdout
import io
import json
from pathlib import Path
import signal
import tempfile
import time
import unittest
from unittest.mock import Mock, patch

import migration_native_run as runner
from migration_native_process import NativeProcessError
from test_migration_native_campaign import artifacts


class RunTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup)
        self.directory = Path(self.temp.name).resolve()
        self.output = self.directory/'campaign'
        self.config = {'input_sha256': 'a'*64, 'artifacts': artifacts(),
                       'inventory_collections': 160, 'archives': {str(i): str(i) for i in range(10)}}
        self.load = patch.object(runner, 'load', return_value=self.config).start()
        self.addCleanup(patch.stopall)
        self.owner = Mock()
        self.owner.teardown.return_value = {'outcome': 'OWNED_INFRASTRUCTURE_REMOVED'}
        self.kind = Mock(return_value=self.owner)
        self.session = Mock()
        self.campaign = Mock()
        self.campaign.return_value.run.return_value = {'outcome': 'NATIVE_CAMPAIGN_OBSERVED'}

    def run_campaign(self, **kwargs):
        return runner.run(self.directory/'inputs', self.output, execute=True,
                          kind_factory=self.kind, session_factory=self.session,
                          campaign_factory=self.campaign, **kwargs)

    def receipt(self):
        return json.loads((self.output/'FINAL-STATUS.json').read_text())

    def test_default_validates_only_and_creates_no_directory(self):
        value = runner.run('input', self.output, kind_factory=self.kind)
        self.assertEqual(value['runtime'], 'NOT_RUN')
        self.assertEqual(value['full_reader_writer_gate'], 'NOT_PROVEN')
        self.kind.assert_not_called();self.assertFalse(self.output.exists())

    def test_success_binds_persistent_child_tick_private_finality_and_restores_timer(self):
        prior = {name: signal.getsignal(name) for name in (signal.SIGALRM, signal.SIGTERM, signal.SIGINT)}
        result = self.run_campaign()
        self.assertEqual(result['outcome'], 'NATIVE_CAMPAIGN_OBSERVED')
        self.assertEqual(result['full_reader_writer_gate'], 'NOT_PROVEN')
        self.campaign.call_args.kwargs['session_factory']()
        self.session.assert_called_once_with(tick=self.owner.pump)
        self.owner.teardown.assert_called_once()
        self.assertEqual(self.receipt(), result)
        self.assertEqual(self.output.stat().st_mode & 0o777, 0o700)
        self.assertEqual((self.output/'FINAL-STATUS.json').stat().st_mode & 0o777, 0o600)
        self.assertEqual(signal.getitimer(signal.ITIMER_REAL), (0., 0.))
        self.assertEqual({name: signal.getsignal(name) for name in prior}, prior)

    def test_interrupted_and_failed_campaigns_preserve_failure_then_teardown(self):
        for exception in (KeyboardInterrupt(), RuntimeError('private failure body')):
            with self.subTest(error=type(exception).__name__):
                self.output = self.directory/type(exception).__name__
                self.campaign.return_value.run.side_effect = exception
                with self.assertRaises(type(exception)): self.run_campaign()
                result = self.receipt()
                self.assertTrue(result['interrupted_or_failed'])
                self.assertEqual(result['outcome'], 'INCOMPLETE')
                self.assertNotIn('private failure body', json.dumps(result))
        self.assertEqual(self.owner.teardown.call_count, 2)

    def test_cleanup_failure_never_reports_observed_and_retains_receipt(self):
        self.owner.teardown.side_effect = RuntimeError('foreign object')
        with self.assertRaisesRegex(NativeProcessError, 'finality'): self.run_campaign()
        self.assertEqual(self.receipt()['outcome'], 'INCOMPLETE')
        self.assertEqual(self.receipt()['infrastructure_finality']['outcome'], 'INCOMPLETE')

    def test_repeated_signal_during_cleanup_is_deferred_until_private_receipt(self):
        calls = []
        def campaign(**_kwargs):
            self.campaign.call_args.kwargs['on_cleanup']()
            signal.raise_signal(signal.SIGTERM)
            calls.append('session finality continued')
            return {'outcome': 'NATIVE_CAMPAIGN_OBSERVED'}
        def teardown():
            signal.raise_signal(signal.SIGINT)
            calls.append('infrastructure finality continued')
            return {'outcome': 'OWNED_INFRASTRUCTURE_REMOVED'}
        self.campaign.return_value.run.side_effect = campaign
        self.owner.teardown.side_effect = teardown
        with self.assertRaises(KeyboardInterrupt): self.run_campaign()
        self.assertEqual(calls, ['session finality continued', 'infrastructure finality continued'])
        self.assertEqual(self.receipt()['outcome'], 'INCOMPLETE')

    def test_real_deadline_interrupts_then_persists_cleanup(self):
        self.campaign.return_value.run.side_effect = lambda **_kwargs: time.sleep(2)
        started = time.monotonic()
        with self.assertRaises(KeyboardInterrupt): self.run_campaign(duration=1)
        self.assertLess(time.monotonic()-started, 2)
        self.owner.teardown.assert_called_once()
        self.assertEqual(self.receipt()['outcome'], 'INCOMPLETE')

    def test_signal_handlers_restore_even_if_final_receipt_cannot_be_written(self):
        prior = signal.getsignal(signal.SIGTERM)
        with patch.object(runner.NativeKind, 'private_write', side_effect=OSError('no space')):
            with self.assertRaises(OSError): self.run_campaign()
        self.assertEqual(signal.getsignal(signal.SIGTERM), prior)
        self.owner.teardown.assert_called_once()

    def test_campaign_failure_is_not_converted_by_successful_teardown(self):
        self.campaign.return_value.run.return_value = {'outcome': 'INCOMPLETE', 'failures': ['failed_boot']}
        self.assertEqual(self.run_campaign()['outcome'], 'INCOMPLETE')
        self.assertEqual(self.receipt()['campaign']['failures'], ['failed_boot'])

    def test_constructor_failure_has_no_infrastructure_and_is_preserved(self):
        self.kind.side_effect = RuntimeError('constructor failure')
        with self.assertRaises(RuntimeError): self.run_campaign()
        self.assertEqual(self.receipt()['infrastructure_finality']['outcome'], 'NO_INFRASTRUCTURE_STARTED')
        self.owner.teardown.assert_not_called()

    def test_existing_timer_is_not_replaced_and_no_infrastructure_starts(self):
        with patch.object(runner.signal, 'getitimer', return_value=(30., 0.)), \
                patch.object(runner.signal, 'setitimer') as timer, patch.object(runner.signal, 'signal') as handler:
            with self.assertRaisesRegex(NativeProcessError, 'existing_timer'): self.run_campaign()
        self.kind.assert_not_called();timer.assert_not_called();handler.assert_not_called()
        self.assertEqual(self.receipt()['outcome'], 'INCOMPLETE')

    def test_invalid_duration_existing_output_and_alias_rejected(self):
        for duration in (0, 7201, True, 1.5):
            with self.assertRaises(NativeProcessError): self.run_campaign(duration=duration)
        self.output.mkdir(mode=0o700)
        with self.assertRaises(NativeProcessError): self.run_campaign()
        self.output.rmdir()
        alias = self.directory/'alias';alias.symlink_to(self.directory)
        self.output = alias/'campaign'
        with self.assertRaises(NativeProcessError): self.run_campaign()
        self.kind.assert_not_called()

    def test_journal_failure_closes_and_preserves_incomplete_summary(self):
        with patch.object(runner.Journal, 'record', side_effect=OSError('disk failure')):
            with self.assertRaises(OSError): self.run_campaign()
        self.kind.assert_not_called()
        self.assertEqual(self.receipt()['outcome'], 'INCOMPLETE')

    def test_cli_default_execution_failure_and_required_output(self):
        for args, outcome, expected in [
            (['--input', 'private'], {'schema': 'test', 'input_sha256': 'a'*64, 'artifacts': artifacts(),
                'runtime': 'NOT_RUN', 'full_reader_writer_gate': 'NOT_PROVEN'}, 0),
            (['--input', 'private', '--execute', '--output', 'private'], {
                'schema': 'test', 'input_sha256': 'a'*64, 'artifacts': artifacts(), 'runtime': 'ATTEMPTED',
                'full_reader_writer_gate': 'NOT_PROVEN', 'outcome': 'INCOMPLETE'}, 1),
            (['--input', 'private'], RuntimeError('private body'), 1),
        ]:
            output = io.StringIO()
            with patch.object(runner, 'run') as run, redirect_stdout(output):
                if isinstance(outcome, Exception): run.side_effect = outcome
                else: run.return_value = outcome
                self.assertEqual(runner.main(args), expected)
            self.assertNotIn('private body', output.getvalue())
            self.assertEqual(json.loads(output.getvalue())['full_reader_writer_gate'], 'NOT_PROVEN')
        with self.assertRaises(SystemExit), patch('sys.stderr', new=io.StringIO()):
            runner.main(['--input', 'private', '--execute'])


if __name__ == '__main__': unittest.main()
