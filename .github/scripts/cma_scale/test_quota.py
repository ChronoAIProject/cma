"""Quota changes must never broaden field scope or lower below current use."""
import copy
import json
from pathlib import Path
import tempfile
from unittest.mock import patch

import controller as c
import operations as o
import rules as g
from test_controller import NoExternalCommands, deployment, prepared, release


def quota(raised=False):
    hard = {'requests.cpu': '40', 'limits.cpu': '80', 'requests.memory': '80Gi',
            'limits.memory': '160Gi', 'pods': '50'}
    if raised:
        hard.update(g.QUOTA_AFTER)
    return {'apiVersion': 'v1', 'kind': 'ResourceQuota',
            'metadata': {'name': 'sandbox-quota', 'namespace': 'chrono-bot',
                         'uid': 'quota-owned', 'resourceVersion': '12'},
            'spec': {'hard': hard},
            'status': {'hard': copy.deepcopy(hard),
                       'used': {'requests.cpu': '8525m', 'limits.cpu': '20250m'}}}


class QuotaTests(NoExternalCommands):
    def test_only_two_CPU_leaves_change_with_UID_RV_and_spec_guards(self):
        before = quota()
        op = o.build('s3-quota-raise', release())
        expected, saved = o.transformed(before, op)
        changes = g.patch_for(before, expected)
        self.assertEqual([x['path'] for x in changes[:3]],
                         ['/metadata/uid', '/metadata/resourceVersion', '/spec'])
        self.assertEqual(changes[3:], [
            {'op': 'replace', 'path': '/spec/hard/requests.cpu', 'value': '64'},
            {'op': 'replace', 'path': '/spec/hard/limits.cpu', 'value': '128'}])
        self.assertEqual(saved[0]['value'], before['spec'])
        for key in ('requests.memory', 'limits.memory', 'pods'):
            self.assertEqual(expected['spec']['hard'][key], before['spec']['hard'][key])
        cmd, body, spec, _ = c.payload(before, op, True)
        self.assertIn('--dry-run=server', cmd)
        self.assertEqual(json.loads(body), changes)
        self.assertEqual(spec, expected['spec'])

    def test_wrong_quota_and_unrelated_fields_and_target_are_refused(self):
        for mutation in ('name', 'field', 'target', 'stage'):
            op = o.build('s3-quota-raise', release())
            if mutation == 'name': op['name'] = 'another-quota'
            if mutation == 'field': op['changes']['cpu']['pods'] = '100'
            if mutation == 'target': op['changes']['cpu']['limits.cpu'] = '256'
            if mutation == 'stage': op['stage'] = 'S1'
            with self.subTest(mutation=mutation), self.assertRaises(RuntimeError):
                o.transformed(quota(), op)

    def test_before_ceiling_drift_refused(self):
        before = quota(); before['spec']['hard']['requests.cpu'] = '48'
        with self.assertRaisesRegex(RuntimeError, 'before_CPU_drift'):
            o.transformed(before, o.build('s3-quota-raise', release()))

    def test_restore_requires_confirmed_owned_raise_and_preserves_before_spec(self):
        before, current = quota(), quota(True)
        # Retain exact original quantity spelling rather than recreate a full manifest.
        before['spec']['hard']['requests.cpu'] = '40000m'
        result = {'state': 'APPLIED', 'operation': o.build('s3-quota-raise', release()),
                  'quota_before_spec': before['spec'], 'after': g.binding(current)}
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'raise.result.json'; g.save(path, result)
            with patch.object(g, 'get', return_value=current):
                op = o.build('s3-quota-restore', release(), created=path)
            restored, _ = o.transformed(current, op)
            self.assertEqual(restored['spec'], before['spec'])
            result['state'] = 'UNKNOWN'; g.save(path, result)
            with self.assertRaisesRegex(RuntimeError, 'confirmed_quota_raise'):
                o.build('s3-quota-restore', release(), created=path)

    def test_restore_refuses_foreign_UID_or_changed_unrelated_quota(self):
        original = quota(True)
        result = {'state': 'APPLIED', 'operation': o.build('s3-quota-raise', release()),
                  'quota_before_spec': quota()['spec'], 'after': g.binding(original)}
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'result.json'; g.save(path, result)
            for field in ('uid', 'pods'):
                current = copy.deepcopy(original)
                if field == 'uid': current['metadata']['uid'] = 'replacement'
                else: current['spec']['hard']['pods'] = '60'
                with patch.object(g, 'get', return_value=current), self.assertRaisesRegex(
                        RuntimeError, 'identity_or_spec_changed'):
                    o.build('s3-quota-restore', release(), created=path)

    def test_restore_rejects_over_usage_missing_usage_or_unreconciled_status(self):
        for key, value in [('requests.cpu', '40001m'), ('limits.cpu', '80001m'),
                           ('requests.cpu', None)]:
            current = quota(True); current['status']['used'][key] = value
            with self.subTest(key=key, value=value), self.assertRaises(RuntimeError):
                c.quota_checkpoint(current, restoring=True)
        current = quota(True); current['status']['hard'] = quota()['spec']['hard']
        with self.assertRaisesRegex(RuntimeError, 'not_reconciled'):
            c.quota_checkpoint(current)
        current = quota(True); current['status']['used'] = dict(g.QUOTA_BEFORE)
        c.quota_checkpoint(current, restoring=True)

    def test_quota_checkpoint_requires_HPAs_absent_and_fixed_pairs(self):
        op = o.build('s3-quota-raise', release())
        with patch.object(g, 'get', return_value={'metadata': {'name': 'hpa'}}), \
                self.assertRaisesRegex(RuntimeError, 'requires_no_HPAs'):
            c.checkpoint(quota(), op, release())
        with patch.object(g, 'get', return_value=None), patch.object(c, 'stable_pair') as pairs:
            c.checkpoint(quota(), op, release())
        self.assertEqual(pairs.call_count, 2)
        self.assertTrue(all(call.kwargs['require_grace'] for call in pairs.call_args_list))

    def test_grace_must_match_both_template_and_each_actual_Pod(self):
        obj = deployment(); obj['spec']['template']['spec']['terminationGracePeriodSeconds'] = 760
        pods = [{'spec': {'terminationGracePeriodSeconds': 760}} for _ in range(2)]
        with patch.object(g, 'get', return_value=obj), \
                patch.object(c.checkpoints, 'stable', return_value=pods):
            c.stable_pair('cma-agent-manager', require_grace=True)
            pods[1]['spec']['terminationGracePeriodSeconds'] = 735
            with self.assertRaisesRegex(RuntimeError, 'grace_760'):
                c.stable_pair('cma-agent-manager', require_grace=True)
            pods[1]['spec']['terminationGracePeriodSeconds'] = 760
            obj['spec']['template']['spec']['terminationGracePeriodSeconds'] = 735
            with self.assertRaisesRegex(RuntimeError, 'grace_760'):
                c.stable_pair('cma-agent-manager', require_grace=True)

    def test_HPA_creation_requires_raised_reconciled_quota(self):
        op = o.build('s3-am-create', release())
        with patch.object(c, 'stable_pair'), patch.object(g, 'get', return_value=quota()), \
                self.assertRaisesRegex(RuntimeError, 'CPU_quota_raise_required'):
            c.checkpoint(None, op)
        with patch.object(c, 'stable_pair'), patch.object(g, 'get', return_value=quota(True)):
            c.checkpoint(None, op)

    def test_approved_quota_write_retains_before_spec_and_verifies_readback(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'quota.plan.json'
            value, after = prepared(path, o.build('s3-quota-raise', release()), quota())
            with patch.object(g, 'get', side_effect=[quota(), quota(), after]), \
                    patch.object(c, 'checkpoint'), patch.object(c, 'write_window'), \
                    patch.object(c, 'policy') as policy, \
                    patch.object(g, 'command', side_effect=[after, after]) as command:
                c.apply(path, 'quota-operation-approval')
            result = g.load(path.with_suffix('.result.json'))
            self.assertEqual(result['quota_before_spec'], quota()['spec'])
            self.assertEqual(result['state'], 'APPLIED')
            self.assertIn('--dry-run=server', command.call_args_list[0].args[0])
            self.assertNotIn('--dry-run=server', command.call_args_list[1].args[0])
            self.assertEqual(policy.call_args_list[1].args,
                             ('quota-operation-approval', value['approval_context']))
