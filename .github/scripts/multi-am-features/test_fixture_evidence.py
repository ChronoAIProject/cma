import json
from pathlib import Path
import tempfile
import unittest

from fixture_evidence import summarize


class FixtureEvidenceTests(unittest.TestCase):
    def test_fixed_roles_keep_states_and_signals_without_private_payload(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / 'pods.final.json').write_text(json.dumps({'items': [
                {'metadata': {'name': 'am-a-0', 'uid': 'private-owner'}, 'status': {
                    'phase': 'Running', 'conditions': [{'type': 'Ready', 'status': 'False',
                        'message': 'private-token'}], 'containerStatuses': [{'restartCount': 3,
                        'state': {'waiting': {'reason': 'CrashLoopBackOff', 'message': 'private'}},
                        'lastState': {'terminated': {'reason': 'Error', 'exitCode': 139,
                            'message': 'private-crash-message'}}}]}},
                {'metadata': {'name': 'private-dynamic-runtime'}, 'status': {'phase': 'Failed'}}]}))
            (root / 'am-a-0.full.private.log').write_text('private-provider-url exec format error')
            (root / 'am-a-0.previous.private.log').write_text('private-crash-message fatal runtime error: stack overflow')
            (root / 'events.final.json').write_text(json.dumps({'items': [
                {'reason': 'Failed', 'message': 'private-ip'}, {'reason': 'private-user'}]}))
            result = summarize(root)
            self.assertTrue(result['roles']['am_a']['observed'])
            self.assertFalse(result['roles']['am_a']['ready'])
            self.assertEqual(result['roles']['am_a']['restarts'], 3)
            self.assertEqual(result['roles']['am_a']['log_signals'], ['executable_format'])
            self.assertEqual(result['roles']['am_a']['previous_termination'], {'reason': 'Error', 'exit_code': 139})
            self.assertEqual(result['roles']['am_a']['previous_log_signals'], ['stack_overflow'])
            self.assertFalse(result['roles']['am_b']['observed'])
            self.assertEqual(result['event_reasons'], {'Failed': 1, 'Other': 1})
            for token in ['private-owner', 'private-token', 'private-provider-url', 'private-user', 'private-ip',
                          'private-crash-message']:
                self.assertNotIn(token, json.dumps(result))
            self.assertFalse(result['root_cause_inferred'])

    def test_malformed_role_does_not_discard_other_role_and_missing_is_not_pass(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            result = summarize(root)
            self.assertFalse(any(v['observed'] for v in result['roles'].values()))
            self.assertNotIn('verdict', result)
            (root / 'pods.final.json').write_text(json.dumps({'items': [
                {'metadata': {'name': 'am-a-0'}, 'status': []},
                {'metadata': {'name': 'mongo'}, 'status': {'phase': 'Running'}}]}))
            result = summarize(root)
            self.assertTrue(result['roles']['am_a']['invalid_pod_sample'])
            self.assertTrue(result['roles']['mongo']['observed'])
            (root / 'pods.final.json').write_text('{invalid')
            self.assertTrue(summarize(root)['pod_sample_invalid'])

    def test_symlinked_capture_is_not_followed(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            target = root / 'private-unrelated'
            target.write_text('private')
            (root / 'pods.final.json').symlink_to(target)
            result = summarize(root)
            self.assertEqual(result['files'], {})
            self.assertFalse(result['roles']['am_a']['observed'])
