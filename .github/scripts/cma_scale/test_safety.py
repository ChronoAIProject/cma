"""Offline regressions for pre-pause identity and unrelated template preservation."""
import argparse
import copy
import json
from pathlib import Path
import tempfile
from unittest.mock import patch

import checkpoints
import controller as c
import operations as o
import rules as g
from test_controller import NoExternalCommands, deployment, prepared, release
from test_recovery import processes


def planned_pause(root):
    original = deployment(1)
    original['spec']['template']['spec']['containers'][0]['env'][1]['value'] = 'false'
    paused, _ = o.transformed(original, o.build('s1-pause', release()))
    rs, pods = processes(original, 1)
    path, release_path = Path(root) / 'pause.json', Path(root) / 'release.json'
    g.save(release_path, release())
    with patch.object(g, 'get', side_effect=lambda kind, name: original if kind == 'Deployment' else None), \
         patch.object(checkpoints, 'cohort', return_value=({'rs': rs}, pods)), \
         patch.object(g, 'command', return_value=paused) as command:
        c.plan(argparse.Namespace(action='s1-pause', release=release_path, out=str(path),
                                 created=None, pause_plan=None, prepare_plan=None))
    assert command.call_count == 1 and '--dry-run=server' in command.call_args.args[0]
    return original, paused, rs, pods, path


class OriginalProcessTests(NoExternalCommands):
    def test_plan_captures_safe_identity_before_pause_and_can_restore(self):
        with tempfile.TemporaryDirectory() as root:
            original, paused, rs, pods, path = planned_pause(root)
            saved = g.load(path)
            identity = saved['operation']['original_process']
            self.assertEqual((identity['deployment_uid'], identity['rs_uid'], identity['pod_uid']),
                             ('dep', 'rs', 'pod-0'))
            self.assertEqual(identity['containers'][0]['container_id'], 'containerd://app-0')
            self.assertNotIn('DO_NOT_PRINT', json.dumps(saved))
            with patch.object(g, 'get', return_value=paused), \
                 patch.object(checkpoints, 'cohort', return_value=({'rs': rs}, pods)):
                op = o.build('s1-abort-before-stop', release(), pause_plan=path)
            self.assertEqual(o.transformed(paused, op)[0]['spec'], original['spec'])

    def test_abort_refuses_recreated_deployment_with_same_spec(self):
        with tempfile.TemporaryDirectory() as root:
            _, paused, _, _, path = planned_pause(root)
            paused['metadata']['uid'] = 'replacement'
            with patch.object(g, 'get', return_value=paused), \
                 patch.object(checkpoints, 'cohort') as cohort, \
                 self.assertRaisesRegex(RuntimeError, 'original_deployment_changed'):
                o.build('s1-abort-before-stop', release(), pause_plan=path)
            cohort.assert_not_called()

    def test_abort_refuses_replaced_pod_rs_and_each_container_incarnation_field(self):
        with tempfile.TemporaryDirectory() as root:
            _, paused, rs, pods, path = planned_pause(root)
            for field in ('pod', 'rs', 'containerID', 'restartCount', 'startedAt', 'imageID'):
                changed, new_rs = copy.deepcopy(pods), copy.deepcopy(rs)
                status = changed[0]['status']['containerStatuses'][0]
                if field == 'pod':
                    changed[0]['metadata']['uid'] = 'replacement'
                elif field == 'rs':
                    new_rs['metadata']['uid'] = 'replacement'
                    changed[0]['metadata']['ownerReferences'][0]['uid'] = 'replacement'
                elif field == 'startedAt':
                    status['state']['running']['startedAt'] = '2026-11-14T00:00:01Z'
                else:
                    status[field] = 1 if field == 'restartCount' else 'replacement'
                with self.subTest(field=field), patch.object(g, 'get', return_value=paused), \
                     patch.object(checkpoints, 'cohort', return_value=({new_rs['metadata']['uid']: new_rs}, changed)), \
                     self.assertRaisesRegex(RuntimeError, 'original_process_changed'):
                    o.build('s1-abort-before-stop', release(), pause_plan=path)

    def test_old_plan_without_original_identity_fails_closed(self):
        with tempfile.TemporaryDirectory() as root:
            _, paused, _, _, path = planned_pause(root)
            saved = g.load(path); del saved['operation']['original_process']; g.save(path, saved)
            with patch.object(g, 'get', return_value=paused), \
                 self.assertRaisesRegex(RuntimeError, 'saved_original_process_required'):
                o.build('s1-abort-before-stop', release(), pause_plan=path)
        with self.assertRaisesRegex(RuntimeError, 'pause_requires_unpaused_legacy_one'):
            g.deployment_checkpoint(deployment(1, True), o.build('s1-pause', release()))

    def test_missing_container_incarnation_refuses_capture(self):
        obj = deployment(1); rs, pods = processes(obj, 1)
        del pods[0]['status']['containerStatuses'][0]['containerID']
        with patch.object(checkpoints, 'cohort', return_value=({'rs': rs}, pods)), \
             self.assertRaisesRegex(RuntimeError, 'original_container_incarnation_required'):
            checkpoints.original_process(obj)

    def test_restart_after_approval_refuses_pause_and_abort_without_write(self):
        for action in ('pause', 'abort'):
            with self.subTest(action=action), tempfile.TemporaryDirectory() as root:
                original, paused, rs, pods, path = planned_pause(root)
                if action == 'abort':
                    with patch.object(g, 'get', return_value=paused), \
                         patch.object(checkpoints, 'cohort', return_value=({'rs': rs}, pods)):
                        op = o.build('s1-abort-before-stop', release(), pause_plan=path)
                    path = Path(root) / 'abort.json'
                    _, expected = prepared(path, operation=op, original=paused)
                    current = paused
                else:
                    expected, current = paused, original
                def policy(*args):
                    if args:
                        status = pods[0]['status']['containerStatuses'][0]
                        status['restartCount'] += 1
                with patch.object(g, 'get', side_effect=lambda kind, name: current if kind == 'Deployment' else None), \
                     patch.object(checkpoints, 'cohort', return_value=({'rs': rs}, pods)), \
                     patch.object(c, 'write_window'), patch.object(c, 'policy', side_effect=policy), \
                     patch.object(g, 'command', return_value=expected) as command, \
                     self.assertRaisesRegex(RuntimeError, 'original_process_changed'):
                    c.apply(path, 'test-only-approval')
                self.assertEqual(command.call_count, 1)
                self.assertIn('--dry-run=server', command.call_args.args[0])
                self.assertFalse(path.with_suffix('.result.json').exists())


class TemplatePreservationTests(NoExternalCommands):
    def test_prepare_preserves_poststart_extra_topology_and_exact_prior_presence(self):
        extra = {'topologyKey': 'example.com/rack', 'whenUnsatisfiable': 'ScheduleAnyway',
                 'maxSkew': 2, 'labelSelector': {'matchLabels': {'other': 'work'}}}
        for stage in ('s1', 's2'):
            for prior in ('absent', 'empty', 'populated'):
                with self.subTest(stage=stage, prior=prior):
                    obj = deployment(1, True); ps = obj['spec']['template']['spec']
                    container = ps['containers'][0]
                    if stage == 's2':
                        container['name'] = 'cmaeg'
                    if prior != 'absent':
                        ps['topologySpreadConstraints'] = []
                        container['lifecycle'] = {}
                    if prior == 'populated':
                        ps['topologySpreadConstraints'] = [copy.deepcopy(extra), {
                            'topologyKey': 'kubernetes.io/hostname', 'whenUnsatisfiable': 'DoNotSchedule',
                            'maxSkew': 3, 'minDomains': 2}]
                        container['lifecycle'] = {'postStart': {'exec': {'command': ['/bin/true']}},
                                                  'preStop': {'sleep': {'seconds': 10}}}
                    original = copy.deepcopy(obj)
                    op = o.build(stage + '-prepare', release()); input_op = copy.deepcopy(op)
                    after, old = o.transformed(obj, op)
                    actual = after['spec']['template']['spec']
                    self.assertEqual(obj, original)
                    self.assertEqual(op, input_op)
                    if prior == 'populated':
                        self.assertEqual(actual['topologySpreadConstraints'][0], extra)
                        host = actual['topologySpreadConstraints'][1]
                        self.assertEqual((host['maxSkew'], host['minDomains']), (1, 2))
                        self.assertEqual(actual['containers'][0]['lifecycle']['postStart'],
                                         container['lifecycle']['postStart'])
                    self.assertEqual(actual['containers'][0]['lifecycle']['preStop'],
                                     op['changes']['container']['lifecycle']['preStop'])
                    restore = {**op, 'abort_before_stop': True, 'restore_fields': [old],
                               'restored_spec_sha256': g.hash_json(original['spec'])}
                    self.assertEqual(o.transformed(after, restore)[0], original)

    def test_ambiguous_topology_and_unrecognized_lifecycle_refuse_preparation(self):
        op = o.build('s1-prepare', release())
        constraint = op['changes']['pod']['topologySpreadConstraints'][0]
        for topology in (None, {}, [constraint, constraint], [{'maxSkew': 1}]):
            obj = deployment(1, True)
            obj['spec']['template']['spec']['topologySpreadConstraints'] = topology
            with self.subTest(topology=topology), self.assertRaisesRegex(RuntimeError, 'topology'):
                o.transformed(obj, op)
        obj = deployment(1, True)
        obj['spec']['template']['spec']['containers'][0]['lifecycle'] = {'unknown': {}}
        with self.assertRaisesRegex(RuntimeError, 'lifecycle_preStop_only'):
            o.transformed(obj, op)
