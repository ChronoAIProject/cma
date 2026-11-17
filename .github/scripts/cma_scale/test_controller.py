import argparse
import copy
import datetime as dt
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import controller as c
import operations as o
import rules as g


def image(role='am', letter='a'):
    repo = 'cma-api' if role == 'am' else 'cma-cmaeg'
    return 'us-central1-docker.pkg.dev/aelf-shared-k8s-prod/aelf-prod-registry/' + repo + '@sha256:' + letter * 64


def release():
    return {'source': 'b' * 40, 'images': {'am': image(), 'eg': image('eg')},
            'am_max_owned_agents': 10,
            'recovery': {r: {'image': image(r, 'c'), 'compatible_mode_on': True} for r in ('am', 'eg')},
            'controller_order_approved': True, 'no_overlap_under_120s_proven': True,
            'hpa': {r: {'handoff_p95_seconds': 90, 'hard_lifetime_seconds': 760,
                        'capacity_processes_per_service': 13 if r == 'am' else 12,
                        'cpu_signal_qualified': True}
                    for r in ('am', 'eg')}}


def deployment(n=2, paused=False):
    return {'metadata': {'uid': 'dep', 'resourceVersion': '9', 'generation': 4},
            'spec': {'replicas': n, 'paused': paused,
                     'selector': {'matchLabels': {'app.kubernetes.io/name': 'cma-agent-manager'}},
                     'template': {'spec': {'containers': [{'name': 'api', 'image': image(),
                         'env': [{'name': 'UNRELATED_PRIVATE_VALUE', 'value': 'DO_NOT_PRINT'},
                                 {'name': 'CHRONO_AGENT_MANAGER_REPLICAS_ENABLED', 'value': 'true'}]}]}}},
            'status': {'observedGeneration': 4, 'replicas': n, 'updatedReplicas': n,
                       'readyReplicas': n, 'availableReplicas': n}}


def prepared(path, operation=None, original=None):
    operation = operation or o.build('s1-one', release())
    original = original or deployment()
    expected, old = o.transformed(original, operation)
    value = {'version': 1, 'route': c.ROUTE, 'release': release(), 'operation': operation,
             'controller_code': c.controller_code(), 'before': g.binding(original),
             'expected_spec_sha256': g.hash_json(expected['spec']), 'previous_fields': old}
    value['approval_context'] = c.context_for(value)
    g.save(path, value)
    return value, expected


class NoExternalCommands(unittest.TestCase):
    def setUp(self):
        for target in ((g, 'command'), (g, 'get'), (subprocess, 'run')):
            mock = patch.object(*target, side_effect=AssertionError('unmocked external command'))
            mock.start()
            self.addCleanup(mock.stop)


class RulesTests(NoExternalCommands):
    def test_native_eg_sleep_and_owned_capacity_bound(self):
        am, eg = (o.build(s + '-prepare', release()) for s in ('s1', 's2'))
        self.assertEqual(eg['changes']['container']['lifecycle'], {'preStop': {'sleep': {'seconds': 30}}})
        self.assertEqual(am['changes']['env'][2]['value'], '10')
        self.assertEqual(eg['changes']['pod']['terminationGracePeriodSeconds'], 760)

    def test_withdrawn_eg_exec_cannot_reenter(self):
        op = o.build('s2-prepare', release())
        obj = deployment(1, True)
        obj['spec']['template']['spec']['containers'][0]['name'] = 'cmaeg'
        op['changes']['container']['lifecycle'] = {'preStop': {'exec': {'command': ['sh']}}}
        with self.assertRaisesRegex(RuntimeError, 'EG_exec_hook_prohibited'):
            g.transform(obj, op)

    def test_missing_release_or_recovery_rejects_cutover(self):
        value = release(); value['recovery']['am']['compatible_mode_on'] = False
        with self.assertRaisesRegex(RuntimeError, 'recovery_must_be_qualified'):
            o.build('s1-stop', value)
        value = release(); value['no_overlap_under_120s_proven'] = False
        with self.assertRaisesRegex(RuntimeError, 'cutover_order'):
            o.build('s1-stop', value)

    def test_hpa_cadence_and_complete_lifetime_capacity(self):
        op = o.build('s3-am-create', release())
        self.assertEqual(op['manifest']['spec']['behavior']['scaleDown']['policies'],
                         [{'type': 'Pods', 'value': 1, 'periodSeconds': 120}])
        for key, bad in [('hard_lifetime_seconds', None), ('capacity_processes_per_service', 5),
                         ('capacity_processes_per_service', 12),
                         ('handoff_p95_seconds', float('nan')), ('handoff_p95_seconds', 100.1),
                         ('handoff_p95_seconds', 1800)]:
            value = release(); value['hpa']['am'][key] = bad
            with self.subTest(key=key), self.assertRaises(RuntimeError):
                o.build('s3-am-create', value)

    def test_role_specific_hpa_envelopes_preserve_eg_reserve(self):
        for role, maximum, processes in [('am', 6, 13), ('eg', 4, 12)]:
            with self.subTest(role=role):
                value = release()
                op = o.build(f's3-{role}-create', value)
                self.assertEqual(op['manifest']['spec']['maxReplicas'], maximum)
                _, _, expected, _ = c.payload(None, op, True)
                self.assertEqual(expected['maxReplicas'], maximum)
                value['hpa'][role]['capacity_processes_per_service'] = processes - 1
                with self.assertRaisesRegex(RuntimeError, 'HPA_capacity_or_CPU_not_qualified'):
                    o.build(f's3-{role}-create', value)
                value['hpa'][role]['capacity_processes_per_service'] = processes
                value['hpa'][role]['hard_lifetime_seconds'] = 841
                with self.assertRaisesRegex(RuntimeError, 'HPA_capacity_or_CPU_not_qualified'):
                    o.build(f's3-{role}-create', value)
                op['manifest']['spec']['maxReplicas'] = 7 if role == 'am' else 6
                with self.assertRaisesRegex(RuntimeError, 'HPA_bounds'):
                    c.payload(None, op, True)

    def test_only_explicit_quiesce_can_remove_multiple_owners(self):
        generic = o.build('s1-one', release()); generic['changes']['deployment']['replicas'] = 0
        with self.assertRaisesRegex(RuntimeError, 'one_owner'):
            g.transform(deployment(), generic)
        op = o.build('s1-quiesce', release())
        target, _ = g.transform(deployment(), op)
        self.assertEqual(target['spec']['replicas'], 0)
        self.assertTrue(target['spec']['paused'])
        self.assertEqual(target['spec']['template'], deployment()['spec']['template'])

    def test_recover_one_needs_actual_zero_and_candidate_rs(self):
        op = o.build('s1-recover-one', release())
        with self.assertRaisesRegex(RuntimeError, 'recovery_requires_zero'):
            g.deployment_checkpoint(deployment(), op)
        obj = deployment(0)
        rs = {'metadata': {'uid': 'rs', 'ownerReferences': [{'uid': 'dep', 'controller': True}]},
              'spec': {'replicas': 0, 'template': copy.deepcopy(obj['spec']['template'])}}
        with patch.object(g, 'command', side_effect=[{'items': []}, {'items': [rs]}]):
            g.deployment_checkpoint(obj, op)
        with patch.object(g, 'command', return_value={'items': [{'metadata': {'deletionTimestamp': 'now'}}]}):
            with self.assertRaisesRegex(RuntimeError, 'old_or_terminating'):
                g.deployment_checkpoint(obj, op)

    def test_unhealthy_owner_is_not_healthy_rollback(self):
        obj = deployment(); obj['status']['readyReplicas'] = 1
        with self.assertRaisesRegex(RuntimeError, 'all_owners'):
            g.deployment_checkpoint(obj, o.build('s1-one', release()))

    def test_blackout_includes_boundary_and_recovery(self):
        for hour, minute in [(1, 58), (2, 0), (2, 59)]:
            with self.assertRaisesRegex(RuntimeError, 'blackout'):
                c.write_window(dt.datetime(2026, 11, 15, hour, minute, tzinfo=c.ZoneInfo('Asia/Singapore')))
        c.write_window(dt.datetime(2026, 11, 15, 3, 0, tzinfo=c.ZoneInfo('Asia/Singapore')))

    def test_patch_guards_identity_rv_generation_and_full_spec(self):
        before = deployment(); after = deployment(1)
        patch = g.patch_for(before, after)
        self.assertEqual([x['path'] for x in patch[:4]],
                         ['/metadata/uid', '/metadata/resourceVersion', '/metadata/generation', '/spec'])
        self.assertEqual(patch[3]['value'], before['spec'])

    def test_healthy_rollback_does_not_save_private_config(self):
        _, old = g.transform(deployment(), o.build('s1-one', release()))
        self.assertNotIn('DO_NOT_PRINT', json.dumps(old))


class TransactionTests(NoExternalCommands):
    def test_apply_uses_current_policy_approval_and_exact_cas(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'plan.json'; value, after = prepared(path)
            before = deployment(); fresh = copy.deepcopy(before); fresh['metadata']['resourceVersion'] = '10'
            with patch.object(g, 'get', side_effect=[before, fresh, after]), \
                 patch.object(c, 'checkpoint') as checkpoint, patch.object(c, 'write_window'), \
                 patch.object(c, 'policy') as policy, \
                 patch.object(g, 'command', side_effect=[after, after]) as command:
                c.apply(path, 'single-use-approval')
            self.assertEqual(policy.call_args_list[1].args,
                             ('single-use-approval', value['approval_context']))
            self.assertEqual(checkpoint.call_count, 2)
            self.assertIn('--dry-run=server', command.call_args_list[0].args[0])
            body = json.loads(command.call_args_list[1].args[1])
            self.assertEqual(body[1]['value'], '10')
            self.assertEqual(g.load(path.with_suffix('.result.json'))['state'], 'APPLIED')

    def test_policy_refusal_never_writes(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'plan.json'; _, after = prepared(path)
            with patch.object(g, 'get', return_value=deployment()), patch.object(c, 'checkpoint'), \
                 patch.object(c, 'write_window'), patch.object(g, 'command', return_value=after) as command, \
                 patch.object(c, 'policy', side_effect=RuntimeError('policy refused')):
                with self.assertRaisesRegex(RuntimeError, 'policy refused'):
                    c.apply(path, 'denied')
            self.assertEqual(command.call_count, 1)
            self.assertFalse(path.with_suffix('.result.json').exists())

    def test_fresh_spec_drift_after_approval_never_writes(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'plan.json'; _, after = prepared(path)
            fresh = deployment(); fresh['spec']['template']['spec']['containers'][0]['image'] = image(letter='d')
            with patch.object(g, 'get', side_effect=[deployment(), fresh]), patch.object(c, 'checkpoint'), \
                 patch.object(c, 'write_window'), patch.object(c, 'policy'), \
                 patch.object(g, 'command', return_value=after) as command:
                with self.assertRaisesRegex(RuntimeError, 'drift'):
                    c.apply(path, 'once')
            self.assertEqual(command.call_count, 1)
            self.assertFalse(path.with_suffix('.result.json').exists())

    def test_status_drift_after_approval_never_writes(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'plan.json'; _, after = prepared(path)
            with patch.object(g, 'get', return_value=deployment()), \
                 patch.object(c, 'checkpoint', side_effect=[None, RuntimeError('owner lost')]), \
                 patch.object(c, 'write_window'), patch.object(c, 'policy'), \
                 patch.object(g, 'command', return_value=after) as command:
                with self.assertRaisesRegex(RuntimeError, 'owner lost'):
                    c.apply(path, 'once')
            self.assertEqual(command.call_count, 1)

    def test_failed_write_is_unknown_and_cannot_repeat(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'plan.json'; _, after = prepared(path)
            with patch.object(g, 'get', return_value=deployment()), patch.object(c, 'checkpoint'), \
                 patch.object(c, 'write_window'), patch.object(c, 'policy'), \
                 patch.object(g, 'command', side_effect=[after, TimeoutError('network lost')]):
                with self.assertRaises(TimeoutError):
                    c.apply(path, 'once')
            self.assertEqual(g.load(path.with_suffix('.result.json'))['state'], 'UNKNOWN')
            with patch.object(g, 'get') as read, self.assertRaisesRegex(RuntimeError, 'previous_attempt'):
                c.apply(path, 'another-approval')
            read.assert_not_called()

    def test_plan_change_invalidates_approval_context(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'plan.json'; value, _ = prepared(path)
            value['release']['source'] = 'e' * 40; g.save(path, value)
            with patch.object(g, 'get') as read, self.assertRaisesRegex(RuntimeError, 'plan_changed'):
                c.apply(path, 'once')
            read.assert_not_called()

    def test_admission_mutation_is_not_silently_accepted(self):
        op = o.build('s1-one', release()); changed = deployment(1)
        changed['spec']['unapproved'] = True
        with patch.object(g, 'command', return_value=changed), \
             self.assertRaisesRegex(RuntimeError, 'admission_change'):
            c.dry_run(deployment(), op)

    def test_policy_route_must_match_not_direct(self):
        result = subprocess.CompletedProcess([], 0, stdout=b'ALLOWED once via owner-authorized-direct')
        with patch.object(c.subprocess, 'run', return_value=result), \
             self.assertRaisesRegex(RuntimeError, 'reviewed_controller'):
            c.policy('once', 'context')


if __name__ == '__main__':
    unittest.main()
