"""Actual process/pipe regressions; no cluster, Docker, Mongo or image execution."""
import hashlib
import json
from pathlib import Path
import secrets
import signal
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

from migration_session_child import SessionChild, SessionChildError, STAGES, positive_seconds


CHILD = '''import json, os, signal, sys, time
mode = None
for line in sys.stdin:
    row = json.loads(line)
    if mode is None:
        mode = row['payload'].get('mode', 'normal')
    if mode == 'hang':
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        while True: time.sleep(.1)
    if mode == 'exit': sys.exit(7)
    if mode == 'stderr':
        sys.stderr.write(row['payload'].get('bearer', ''))
        sys.stderr.flush()
    if mode == 'line': os.write(1,b'x'*10000); continue
    if mode == 'total': os.write(2,b'x'*10000); continue
    if mode == 'invalid': os.write(1,b'bad-json\\n'); continue
    if mode == 'utf8': os.write(1,b'\\xff\\n'); continue
    if mode == 'empty': os.write(1,b'\\n'); continue
    if mode == 'list': os.write(1,b'[]\\n'); continue
    if mode == 'fail':
        print(json.dumps({'op':'failed','reason':row['payload'].get('bearer')}),flush=True)
        break
    if mode == 'late': time.sleep(.2)
    op={'initialize':'initialized','stage':'stage','finish':'finished'}[row['op']]
    response={'sequence':row['sequence'],'op':op}
    if row['op']=='initialize': response['outcome']='SESSION_INITIALIZED'
    else: response['result']={'outcome':'SESSION_OBSERVED'}
    if mode=='wrong_sequence': response['sequence'] += 1
    if mode=='wrong_op': response['op']='other'
    if mode=='bad_init': response['outcome']='invented'
    if mode=='bad_result' and row['op']=='stage': response['result']='not-dict'
    if mode=='duplicate': print(json.dumps(response)+'\\n'+json.dumps(response),flush=True)
    else: print(json.dumps(response),flush=True)
    if mode=='trailing':
        time.sleep(.05); os.write(1,b'partial'); break
    if row['op']=='finish':
        if mode=='descendant':
            child=os.fork()
            if child==0:
                signal.signal(signal.SIGTERM,signal.SIG_DFL)
                time.sleep(20);os._exit(0)
        break
'''


class ChildTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.path = Path(self.temp.name) / 'child.py'
        self.path.write_text(CHILD)

    def child(self, **kwargs):
        child = SessionChild(node=sys.executable, protocol=self.path, timeout=2,
                             cleanup_timeout=.5, **kwargs)
        self.addCleanup(child.close)
        child.start()
        return child

    def full(self, child, mode='normal'):
        child.command('initialize', {'mode': mode})
        for stage in STAGES:
            child.command('stage', {'stage': stage})
        child.command('finish', {})
        return child.close()

    def test_complete_same_child_and_idempotent_finality(self):
        child = self.child()
        pid = child.process.pid
        receipt = self.full(child)
        self.assertEqual(child.process.pid, pid)
        self.assertEqual(receipt['outcome'], 'SESSION_OBSERVED')
        self.assertEqual(receipt['commands_completed'], 6)
        self.assertTrue(receipt['processes']['process_group_cleanup'])
        self.assertIs(child.close(), receipt)
        with self.assertRaises(SessionChildError): child.command('finish', {})
        with self.assertRaises(SessionChildError): child.start()

    def test_transport_tick_during_commands_is_suppressed_after_cleanup_starts(self):
        ticks = []
        child = self.child(tick=lambda: ticks.append('drained'))
        child.command('initialize', {})
        self.assertTrue(ticks)
        def dead_forward(): raise RuntimeError('forward unavailable')
        child.tick = dead_forward
        with self.assertRaises(SessionChildError): child.command('stage', {'stage': STAGES[0]})
        # The same child can see EOF and finish; a failed external forward
        # must not prevent bounded process cleanup or imply a replay.
        receipt = child.close()
        self.assertEqual(receipt['outcome'], 'INCOMPLETE')
        self.assertTrue(receipt['processes']['process_group_cleanup'])

    def test_real_node_protocol_and_journey_in_one_subprocess(self):
        root = Path(__file__).parent
        child = SessionChild(protocol=root / 'migration_session_child_fixture.mjs', timeout=5,
                             cleanup_timeout=2).start()
        self.addCleanup(child.close)
        code = ('import {stageCommand} from ' + json.dumps((root / 'migration_agent_session_fixture.mjs').as_uri())
                + '; console.log(JSON.stringify(' + json.dumps(list(STAGES)) + '.map(stageCommand)));')
        import subprocess
        commands = json.loads(subprocess.check_output(['node', '--input-type=module', '-e', code], text=True))
        child.command('initialize', {})
        for command in commands:
            response = child.command('stage', command)
            self.assertEqual(response['result']['stage'], command['stage'])
        child.command('finish', {})
        receipt = child.close()
        self.assertEqual(receipt['outcome'], 'SESSION_OBSERVED')
        final = receipt['responses'][-1]['result']
        self.assertEqual([r['responses'] for r in final['stages'][-1]['agents']], [4, 2])
        self.assertEqual(final['full_reader_writer_gate'], 'NOT_PROVEN')

    def test_rejected_real_protocol_has_nonzero_finality(self):
        child = SessionChild(timeout=5, cleanup_timeout=2).start()
        self.addCleanup(child.close)
        with self.assertRaises(SessionChildError): child.command('initialize', {})
        receipt = child.close()
        self.assertEqual(receipt['outcome'], 'INCOMPLETE')
        self.assertEqual(receipt['exit_code'], 1)
        self.assertTrue(receipt['processes']['process_group_cleanup'])

    def test_timeout_late_ack_never_unpoisons_or_replays(self):
        child = self.child()
        child.timeout = .05
        with self.assertRaises(SessionChildError): child.command('initialize', {'mode': 'late'})
        with self.assertRaises(SessionChildError): child.command('initialize', {})
        receipt = child.close()
        self.assertEqual(receipt['outcome'], 'INCOMPLETE')
        self.assertEqual(receipt['commands_completed'], 0)
        self.assertEqual(len(receipt['responses']), 1)

    def test_deadline_is_bounded_and_stubborn_child_is_killed(self):
        child = self.child()
        child.timeout = .1
        before = time.monotonic()
        with self.assertRaises(SessionChildError): child.command('initialize', {'mode': 'hang'})
        receipt = child.close()
        self.assertLess(time.monotonic()-before, 5)
        self.assertEqual(receipt['exit_code'], -signal.SIGKILL)
        self.assertTrue(receipt['processes']['group_kill_sent'])
        self.assertTrue(receipt['processes']['process_group_cleanup'])

    def test_descendant_pipe_custody_and_parent_exit(self):
        child = self.child()
        receipt = self.full(child, 'descendant')
        self.assertEqual(receipt['outcome'], 'INCOMPLETE')
        self.assertIn('session_child_finality_unconfirmed', receipt['errors'])
        self.assertTrue(receipt['processes']['group_term_sent'])
        self.assertTrue(receipt['processes']['process_group_cleanup'])

    def test_credentials_do_not_enter_receipt_or_argv(self):
        credential = 'ephemeral-"quoted"-\\-' + secrets.token_urlsafe(24)
        child = self.child()
        child.command('initialize', {'mode': 'stderr', 'bearer': credential})
        receipt = child.close()
        self.assertEqual(receipt['stderr_sha256'], hashlib.sha256(credential.encode()).hexdigest())
        self.assertNotIn(credential, json.dumps(receipt))
        self.assertNotIn(credential, ' '.join(child.argv))
        failed = self.child()
        with self.assertRaises(SessionChildError):
            failed.command('initialize', {'mode': 'fail', 'bearer': credential})
        self.assertNotIn('ephemeral-', json.dumps(failed.close()))

    def test_protocol_failure_modes_poison_connection(self):
        for mode in ('exit', 'invalid', 'utf8', 'empty', 'list', 'wrong_sequence',
                     'wrong_op', 'bad_init', 'duplicate'):
            with self.subTest(mode=mode):
                child = self.child()
                with self.assertRaises(SessionChildError): child.command('initialize', {'mode': mode})
                with self.assertRaises(SessionChildError): child.command('initialize', {})
                receipt = child.close()
                self.assertEqual(receipt['outcome'], 'INCOMPLETE')
                self.assertTrue(receipt['processes']['process_group_cleanup'])

    def test_output_bounds_and_truncation(self):
        for mode, bound in [('line', 'LINE_LIMIT'), ('total', 'OUTPUT_LIMIT')]:
            with self.subTest(mode=mode), patch('migration_session_child.' + bound, 100):
                child = self.child()
                with self.assertRaises(SessionChildError): child.command('initialize', {'mode': mode})
                self.assertEqual(child.close()['outcome'], 'INCOMPLETE')
        child = self.child()
        child.command('initialize', {'mode': 'trailing'})
        self.assertIn('session_child_truncated_output', child.close()['errors'])

    def test_input_sequence_bounds_and_invalid_json_numbers(self):
        for op, payload in [('stage', {}), ('initialize', []), ('initialize', {'v': float('nan')}),
                            ('initialize', {'v': 'x' * (256*1024)})]:
            child = self.child()
            with self.assertRaises(SessionChildError): child.command(op, payload)
            self.assertTrue(child.close()['ambiguous_or_failed_command'])
        child = self.child()
        child.command('initialize', {})
        with self.assertRaises(SessionChildError): child.command('stage', {'stage': 'new-write'})
        child.close()
        child = self.child()
        child.command('initialize', {'mode': 'bad_result'})
        with self.assertRaises(SessionChildError): child.command('stage', {'stage': STAGES[0]})
        child.close()

    def test_invalid_deadlines_and_start_failure(self):
        for value in (0, -1, True, None, float('inf'), float('nan'), 3001):
            with self.assertRaises(SessionChildError): positive_seconds(value, 300)
        child = SessionChild(node='/missing/native-campaign-node', cleanup_timeout=.1)
        with self.assertRaises(SessionChildError): child.start()
        self.assertTrue(child.close()['processes']['process_group_cleanup'])
        unopened = SessionChild()
        with self.assertRaises(SessionChildError): unopened.command('initialize', {})
        self.assertEqual(unopened.close()['outcome'], 'INCOMPLETE')


if __name__ == '__main__':
    unittest.main()
