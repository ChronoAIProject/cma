import copy
import datetime as dt
import json
from pathlib import Path
import tempfile
from unittest.mock import patch

import checkpoints
import controller as c
import observe
import operations as o
import rules as g
from test_controller import NoExternalCommands, deployment, image, prepared, release


def processes(obj, count=2):
    rs = {'metadata': {'uid': 'rs', 'ownerReferences': [{'uid': 'dep', 'controller': True}]},
          'spec': {'replicas': count, 'template': copy.deepcopy(obj['spec']['template'])}}
    pods = [{'metadata': {'name': 'pod-' + str(i), 'uid': 'pod-' + str(i),
                         'labels': dict(obj['spec']['selector']['matchLabels']),
                         'ownerReferences': [{'uid': 'rs', 'controller': True}]},
             'spec': {'containers': copy.deepcopy(obj['spec']['template']['spec']['containers']),
                      'nodeName': 'host-' + str(i)},
             'status': {'conditions': [{'type': 'Ready', 'status': 'True'}],
                        'containerStatuses': [{'name': 'api', 'ready': True,
                                               'imageID': image(), 'restartCount': 0,
                                               'containerID': 'containerd://app-' + str(i),
                                               'state': {'running': {'startedAt': '2026-11-14T00:00:00Z'}}}]}}
            for i in range(count)]
    return rs, pods


class RecoveryTests(NoExternalCommands):
    def test_unfilled_release_example_refuses_before_external_calls(self):
        with self.assertRaisesRegex(RuntimeError, 'final_source_required'):
            o.release(Path(o.__file__).with_name('release.example.json'))

    def test_create_dry_run_uses_strict_server_validation(self):
        for action in ('s1-pdb', 's2-pdb', 's3-am-create', 's3-eg-create'):
            op = o.build(action, release())
            cmd, body, expected, _ = c.payload(None, op, True)
            self.assertIn('--dry-run=server', cmd)
            self.assertIn('--validate=strict', cmd)
            self.assertEqual(json.loads(body)['spec'], expected)
            with self.assertRaisesRegex(RuntimeError, 'create_conflict'):
                c.payload({'spec': expected}, op, True)

    def test_recovery_unpause_accepts_only_qualified_compatible_image(self):
        obj = deployment(0, True)
        obj['spec']['template']['spec']['containers'][0]['image'] = image(letter='c')
        with patch.object(g, 'get', return_value=None), \
             patch.object(g, 'deployment_checkpoint'), \
             patch.object(checkpoints, 'cohort', return_value=({}, [])):
            c.checkpoint(obj, o.build('s1-unpause', release()), release())
            obj['spec']['template']['spec']['containers'][0]['image'] = image(letter='e')
            with self.assertRaisesRegex(RuntimeError, 'outside_selected_release'):
                c.checkpoint(obj, o.build('s1-unpause', release()), release())

    def pause_prepare(self, root):
        original = deployment(1)
        original['spec']['template']['spec']['containers'][0]['env'][1]['value'] = 'false'
        pause = o.build('s1-pause', release())
        rs, pods = processes(original, 1)
        with patch.object(checkpoints, 'cohort', return_value=({'rs': rs}, pods)):
            pause['original_process'] = checkpoints.original_process(original)
        paused, old = o.transformed(original, pause)
        first = {'operation': pause, 'release': release(), 'before': g.binding(original),
                 'expected_spec_sha256': g.hash_json(paused['spec']), 'previous_fields': old}
        prepare = o.build('s1-prepare', release())
        current, old = o.transformed(paused, prepare)
        second = {'operation': prepare, 'release': release(), 'before': g.binding(paused),
                  'expected_spec_sha256': g.hash_json(current['spec']), 'previous_fields': old}
        p1, p2 = Path(root) / 'pause.json', Path(root) / 'prepare.json'
        g.save(p1, first); g.save(p2, second)
        return original, current, p1, p2

    def test_pre_stop_abort_restores_original_without_exporting_private_spec(self):
        with tempfile.TemporaryDirectory() as tmp:
            original, current, p1, p2 = self.pause_prepare(tmp)
            rs, pods = processes(original, 1)
            with patch.object(g, 'get', return_value=current), \
                 patch.object(g, 'command', side_effect=[{'items': [rs]}, {'items': pods}]):
                op = o.build('s1-abort-before-stop', release(), pause_plan=p1, prepare_plan=p2)
            restored, _ = o.transformed(current, op)
            self.assertEqual(restored['spec'], original['spec'])
            self.assertNotIn('DO_NOT_PRINT', json.dumps(op))

    def test_pre_stop_abort_refuses_after_stop_and_foreign_process(self):
        with tempfile.TemporaryDirectory() as tmp:
            original, current, p1, p2 = self.pause_prepare(tmp)
            stopped = copy.deepcopy(current); stopped['spec']['replicas'] = 0
            with patch.object(g, 'get', return_value=stopped), \
                 self.assertRaisesRegex(RuntimeError, 'only_before_old_stop'):
                o.build('s1-abort-before-stop', release(), pause_plan=p1, prepare_plan=p2)
            rs, pods = processes(original, 1)
            pods[0]['metadata']['ownerReferences'][0]['uid'] = 'different-rs'
            with patch.object(g, 'get', return_value=current), \
                 patch.object(g, 'command', side_effect=[{'items': [rs]}, {'items': pods}]), \
                 self.assertRaisesRegex(RuntimeError, 'foreign_pod'):
                o.build('s1-abort-before-stop', release(), pause_plan=p1, prepare_plan=p2)

    def test_hpa_freeze_delete_requires_exact_created_uid(self):
        op = o.build('s3-am-create', release())
        obj = {'metadata': {'uid': 'hpa', 'generation': 1, 'resourceVersion': '4'},
               'spec': op['manifest']['spec']}
        result = {'state': 'APPLIED', 'operation': op, 'after': g.binding(obj)}
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'created.json'; g.save(path, result)
            with patch.object(g, 'get', return_value=obj):
                freeze = o.build('s3-am-freeze', release(), created=path)
                with self.assertRaisesRegex(RuntimeError, 'freeze_both'):
                    o.build('s3-am-delete', release(), created=path)
            frozen, _ = o.transformed(obj, freeze)
            with patch.object(g, 'get', return_value=frozen):
                delete = o.build('s3-am-delete', release(), created=path)
            _, body = g.delete_request(frozen, delete, True)
            options = json.loads(body)
            self.assertEqual(options['preconditions'], {'uid': 'hpa', 'resourceVersion': '4'})
            self.assertEqual(options['dryRun'], ['All'])
            obj['metadata']['uid'] = 'recreated'
            with patch.object(g, 'get', return_value=obj), \
                 self.assertRaisesRegex(RuntimeError, 'only_exact_stage_created'):
                o.build('s3-am-freeze', release(), created=path)

    def test_descent_is_one_at_a_time_and_stops_at_two(self):
        with patch.object(g, 'get', side_effect=[None, deployment(5)]):
            op = o.build('s3-am-descend', release())
        self.assertEqual(op['changes']['deployment']['replicas'], 4)
        for count in (2, 1):
            with patch.object(g, 'get', side_effect=[None, deployment(count)]), \
                 self.assertRaisesRegex(RuntimeError, 'stops_at_two'):
                o.build('s3-am-descend', release())

    def test_downscale_requires_recent_survivor_capacity(self):
        op = o.build('s1-one', release())
        for age, minimum, passes in ((121, 1, False), (1, 2, False), (1, 1, True)):
            value = release()
            value['survivor_capacity'] = {'am': {
                'observed_at': (dt.datetime.now(dt.timezone.utc) - dt.timedelta(seconds=age)).isoformat(),
                'qualified': True, 'minimum_safe_replicas': minimum}}
            with patch.object(g, 'get', return_value=None), \
                 patch.object(g, 'deployment_checkpoint'), patch.object(checkpoints, 'stable'):
                if passes:
                    c.checkpoint(deployment(), op, value)
                else:
                    with self.assertRaisesRegex(RuntimeError, 'survivor_capacity'):
                        c.checkpoint(deployment(), op, value)

    def test_new_hpa_between_plan_and_apply_blocks_deployment_write(self):
        with patch.object(g, 'get', return_value={'metadata': {'uid': 'new-hpa'}}), \
             self.assertRaisesRegex(RuntimeError, 'remove_both_HPAs'):
            c.checkpoint(deployment(), o.build('s1-one', release()))

    def test_controller_reservation_race_stops_before_mutation(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'plan.json'; _, after = prepared(path)
            def policy(*args):
                if args:
                    g.save(path.with_suffix('.result.json'), {'state': 'WRITE_ATTEMPTED'})
            with patch.object(g, 'get', return_value=deployment()), patch.object(c, 'checkpoint'), \
                 patch.object(c, 'write_window'), patch.object(c, 'policy', side_effect=policy), \
                 patch.object(g, 'command', return_value=after) as command, \
                 self.assertRaises(FileExistsError):
                c.apply(path, 'once')
            self.assertEqual(command.call_count, 1)  # Only the server dry-run.


class CohortTests(NoExternalCommands):
    def test_stable_pair_requires_owned_current_image_and_actual_hosts(self):
        obj = deployment(); rs, pods = processes(obj)
        with patch.object(g, 'command', side_effect=[{'items': [rs]}, {'items': pods}]):
            self.assertEqual(len(checkpoints.stable(obj, 2, distinct=True)), 2)
        cases = [('foreign', 'foreign_pod'), ('image', 'running_image'),
                 ('terminating', 'actual_termination'), ('host', 'distinct_host')]
        for case, message in cases:
            broken = copy.deepcopy(pods)
            if case == 'foreign':
                broken[0]['metadata']['ownerReferences'][0]['uid'] = 'other'
            elif case == 'image':
                broken[0]['status']['containerStatuses'][0]['imageID'] = image(letter='f')
            elif case == 'terminating':
                broken[0]['metadata']['deletionTimestamp'] = 'now'
            else:
                broken[0]['spec']['nodeName'] = broken[1]['spec']['nodeName']
            with self.subTest(case=case), \
                 patch.object(g, 'command', side_effect=[{'items': [rs]}, {'items': broken}]), \
                 self.assertRaisesRegex(RuntimeError, message):
                checkpoints.stable(obj, 2, distinct=True)

    def test_wrong_label_does_not_hide_owned_terminating_pod(self):
        obj = deployment(); rs, pods = processes(obj)
        extra = copy.deepcopy(pods[0]); extra['metadata']['labels'] = {}
        extra['metadata']['deletionTimestamp'] = 'now'
        with patch.object(g, 'command', side_effect=[{'items': [rs]}, {'items': pods + [extra]}]), \
             self.assertRaisesRegex(RuntimeError, 'actual_termination'):
            checkpoints.stable(obj, 2)

    def test_outage_boundary_is_strict_and_never_readiness(self):
        start = '2026-01-01T00:00:00Z'
        self.assertEqual(observe.outage(start, '2026-01-01T00:01:59Z')['state'], 'RESTORED')
        self.assertEqual(observe.outage(start, '2026-01-01T00:02:00Z')['state'], 'BREACHED')
        self.assertTrue(observe.outage(start, '2026-01-01T00:01:59Z')['service_evidence_required'])

    def test_snapshot_preserves_retired_restart_history(self):
        am = deployment(1); _, pods = processes(am, 1)
        previous = {'context': g.CONTEXT, 'namespace': g.NAMESPACE, 'pods': [
            {'uid': 'pod-0', 'restart_count': 0, 'lifecycle': 'active'}]}
        pods[0]['status']['containerStatuses'][0]['restartCount'] = 1
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'history.json'; g.save(path, previous)
            with patch.object(g, 'get', return_value=am), \
                 patch.object(checkpoints, 'cohort', side_effect=[({}, pods), ({}, [])]):
                result = observe.snapshot(release(), 'S1', 'run', '2026-01-01T00:00:00Z', path)
            old = result['pods'][-1]
            self.assertEqual(old['lifecycle'], 'retired')
            self.assertEqual(old['restart_count'], 0)
            self.assertIn('observed_until_utc', old)
            self.assertNotIn('DO_NOT_PRINT', json.dumps(result))

    def test_retired_source_reappearance_is_not_silently_unretired(self):
        am = deployment(1); _, pods = processes(am, 1)
        previous = {'context': g.CONTEXT, 'namespace': g.NAMESPACE, 'pods': [
            {'uid': 'pod-0', 'restart_count': 0, 'lifecycle': 'retired'}]}
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'history.json'; g.save(path, previous)
            with patch.object(g, 'get', return_value=am), \
                 patch.object(checkpoints, 'cohort', side_effect=[({}, pods), ({}, [])]), \
                 self.assertRaisesRegex(RuntimeError, 'retired_source_observed_again'):
                observe.snapshot(release(), 'S1', 'run', '2026-01-01T00:00:00Z', path)
