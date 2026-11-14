"""Regression for production API omission of DeploymentSpec.paused=false."""
import copy
import json
from pathlib import Path
import tempfile
from unittest.mock import patch

import controller as c
import operations as o
import rules as g
from test_controller import NoExternalCommands, deployment, image, prepared, release


def paused_zero(role):
    obj = deployment(0, True)
    container = obj['spec']['template']['spec']['containers'][0]
    container['name'] = 'api' if role == 'am' else 'cmaeg'
    container['image'] = image(role)
    return obj


class UnpauseAdmissionTests(NoExternalCommands):
    def test_both_roles_emit_absent_paused_and_preserve_every_other_field(self):
        for role, stage in (('am', 's1'), ('eg', 's2')):
            with self.subTest(role=role):
                before = paused_zero(role)
                op = o.build(stage + '-unpause', release())
                expected = copy.deepcopy(before); del expected['spec']['paused']
                command, body, spec, old = c.payload(before, op, True)
                self.assertIn('--dry-run=server', command)
                self.assertEqual(spec, expected['spec'])
                self.assertTrue(before['spec']['paused'])
                self.assertEqual(old, [{'path': ['spec', 'paused'], 'present': True, 'value': True}])
                rows = json.loads(body)
                self.assertEqual(rows[3], {'op': 'test', 'path': '/spec', 'value': before['spec']})
                self.assertNotIn('paused', rows[-1]['value'])
                with patch.object(g, 'command', return_value=expected):
                    self.assertEqual(c.dry_run(before, op)[0], expected['spec'])

    def test_unrelated_admission_change_still_refuses(self):
        before = paused_zero('am'); op = o.build('s1-unpause', release())
        admitted = copy.deepcopy(before); del admitted['spec']['paused']
        admitted['spec']['template']['spec']['containers'][0]['image'] = image(letter='f')
        with patch.object(g, 'command', return_value=admitted), \
             self.assertRaisesRegex(RuntimeError, 'server_default_or_admission_change'):
            c.dry_run(before, op)

    def test_apply_readback_accepts_API_unpaused_shape_without_relaxing_CAS(self):
        for role, stage in (('am', 's1'), ('eg', 's2')):
            with self.subTest(role=role), tempfile.TemporaryDirectory() as root:
                before = paused_zero(role); op = o.build(stage + '-unpause', release())
                path = Path(root) / 'unpause.json'
                _, expected = prepared(path, operation=op, original=before)
                self.assertNotIn('paused', expected['spec'])
                fresh = copy.deepcopy(before); fresh['metadata']['resourceVersion'] = '10'
                with patch.object(g, 'get', side_effect=[before, fresh, expected]), \
                     patch.object(c, 'checkpoint'), patch.object(c, 'policy'), \
                     patch.object(c, 'write_window'), \
                     patch.object(g, 'command', side_effect=[expected, expected]) as command:
                    c.apply(path, 'offline-test-only')
                rows = json.loads(command.call_args_list[-1].args[1])
                self.assertEqual(rows[1]['value'], '10')
                self.assertEqual(rows[3]['value'], before['spec'])
                self.assertEqual(g.load(path.with_suffix('.result.json'))['state'], 'APPLIED')

    def test_pre_stop_restore_still_preserves_original_absence(self):
        before = deployment(1); del before['spec']['paused']
        paused, old = o.transformed(before, o.build('s1-pause', release()))
        self.assertEqual(old, [{'path': ['spec', 'paused'], 'present': False, 'value': None}])
        restore = {'abort_before_stop': True, 'name': 'cma-agent-manager',
                   'restore_fields': [old], 'restored_spec_sha256': g.hash_json(before['spec'])}
        self.assertEqual(o.transformed(paused, restore)[0], before)

    def test_unrelated_postwrite_drift_is_unknown_and_not_retried(self):
        with tempfile.TemporaryDirectory() as root:
            before = paused_zero('am'); op = o.build('s1-unpause', release())
            path = Path(root) / 'unpause.json'
            _, expected = prepared(path, operation=op, original=before)
            changed = copy.deepcopy(expected); changed['spec']['minReadySeconds'] = 99
            with patch.object(g, 'get', side_effect=[before, before, changed]), \
                 patch.object(c, 'checkpoint'), patch.object(c, 'policy'), \
                 patch.object(c, 'write_window'), \
                 patch.object(g, 'command', side_effect=[expected, expected]), \
                 self.assertRaisesRegex(RuntimeError, 'write_result_uncertain'):
                c.apply(path, 'offline-test-only')
            self.assertEqual(g.load(path.with_suffix('.result.json'))['state'], 'UNKNOWN')
            with self.assertRaisesRegex(RuntimeError, 'previous_attempt'):
                c.apply(path, 'cannot-retry')

    def test_non_boolean_pause_request_refuses_before_external_commands(self):
        for value in (0, 1, None, 'false'):
            op = o.build('s1-unpause', release())
            op['changes']['deployment']['paused'] = value
            with self.subTest(value=value), self.assertRaisesRegex(RuntimeError, 'pause_boolean'):
                c.payload(paused_zero('am'), op, True)
