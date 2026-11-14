"""No-network S3b guards, single-leaf mutation and exact recovery."""
import copy
import datetime as dt
import json
from pathlib import Path
import tempfile
from unittest.mock import patch

import checkpoints
import controller as c
import hpa_exercise as e
import operations as o
import rules as g
from test_controller import NoExternalCommands, deployment, image, prepared, release
from test_recovery import processes


def hpa(role='am', value=30):
    return {'metadata': {'uid': 'hpa-' + role, 'resourceVersion': '10', 'generation': 1},
            'spec': e.original_spec(role), 'status': {'observedGeneration': 1,
                'conditions': [{'type': k, 'status': 'True'} for k in ('AbleToScale', 'ScalingActive')],
                'currentMetrics': [{'type': 'Resource', 'resource': {
                    'name': 'cpu', 'current': {'averageUtilization': value}}}]}}


def prior(root, obj, role='am', operation=None):
    path = Path(root) / 'prior.json'
    g.save(path, {'state': 'APPLIED', 'source': release()['source'],
                  'operation': operation or o.build('s3-' + role + '-create', release()),
                  'after': g.binding(obj)})
    return path


def lower(root, role='am', value=30):
    obj = hpa(role, value); path = prior(root, obj, role)
    with patch.object(g, 'get', return_value=obj), \
         patch.object(e, 'stable_pairs', return_value=({'am': {}, 'eg': {}}, {role: []})), \
         patch.object(e, 'fresh_metrics'):
        op = o.build('s3-' + role + '-lower-target', release(), created=path)
    return obj, op


class ExerciseTests(NoExternalCommands):
    def test_lower_and_restore_only_CPU_leaf_with_fresh_full_guards(self):
        for role in ('am', 'eg'):
            with self.subTest(role=role), tempfile.TemporaryDirectory() as root:
                obj, op = lower(root, role)
                command, body, spec, _ = c.payload(obj, op, True)
                rows = json.loads(body)
                self.assertIn('--dry-run=server', command)
                self.assertEqual([r['path'] for r in rows[:4]],
                                 ['/metadata/uid', '/metadata/resourceVersion', '/metadata/generation', '/spec'])
                self.assertEqual(rows[-1], {'op': 'replace', 'path': e.TARGET_PATH, 'value': 25})
                after = copy.deepcopy(obj); after['spec'] = spec
                after['metadata'].update(generation=2, resourceVersion='11')
                path = prior(root, after, role, op)
                after['metadata']['resourceVersion'] = '99'  # Status churn is permitted.
                after['status'] = {}  # Restore must work during metrics failure.
                with patch.object(g, 'get', return_value=after):
                    restore = o.build('s3-' + role + '-restore-target', release(), created=path)
                e.checkpoint(after, restore, release())
                _, body, spec, _ = c.payload(after, restore, True)
                self.assertEqual(spec, obj['spec'])
                self.assertEqual(json.loads(body)[1]['value'], '99')
                self.assertEqual(json.loads(body)[-1]['value'], 60 if role == 'am' else 200)

    def test_low_unknown_or_not_lower_utilization_refuses(self):
        for value in (None, True, 0, 5, 11, 72, 100):
            with self.subTest(value=value), tempfile.TemporaryDirectory() as root, \
                 self.assertRaisesRegex(RuntimeError, 'S3b_'):
                lower(root, value=value)

    def test_result_source_uid_and_full_spec_drift_refuse(self):
        for field in ('source', 'uid', 'policy'):
            with self.subTest(field=field), tempfile.TemporaryDirectory() as root:
                obj = hpa(); path = prior(root, obj)
                if field == 'source':
                    result = g.load(path); result['source'] = 'f' * 40; g.save(path, result)
                elif field == 'uid':
                    obj['metadata']['uid'] = 'replaced'
                else:
                    obj['spec']['behavior']['scaleDown']['policies'][0]['periodSeconds'] = 60
                with patch.object(g, 'get', return_value=obj), self.assertRaises(RuntimeError):
                    o.build('s3-am-lower-target', release(), created=path)

    def test_restore_refuses_changed_spec_or_unconfirmed_lower(self):
        for field in ('state', 'target', 'other_policy', 'uid'):
            with self.subTest(field=field), tempfile.TemporaryDirectory() as root:
                obj, op = lower(root)
                after, _ = o.transformed(obj, op)
                path = prior(root, after, operation=op)
                if field == 'state':
                    result = g.load(path); result['state'] = 'UNKNOWN'; g.save(path, result)
                elif field == 'target':
                    after['spec']['metrics'][0]['resource']['target']['averageUtilization'] = 24
                elif field == 'other_policy':
                    after['spec']['maxReplicas'] = 4
                else:
                    after['metadata']['uid'] = 'replacement'
                with patch.object(g, 'get', return_value=after), self.assertRaises(RuntimeError):
                    o.build('s3-am-restore-target', release(), created=path)

    def test_stable_pairs_require_original_other_target_ready_duration_and_grace(self):
        objects, pods = {}, {}
        for role, name in (('am', 'cma-agent-manager'), ('eg', 'cma-event-gateway')):
            obj = deployment(); ps = obj['spec']['template']['spec']
            ps['terminationGracePeriodSeconds'] = 760
            ps['containers'][0].update(name=g.TARGETS[name], image=image(role))
            ps['containers'][0]['resources'] = {'requests': {'cpu': '4' if role == 'am' else '250m'}}
            objects[name] = obj
            _, rows = processes(obj)
            for row in rows:
                row['spec']['terminationGracePeriodSeconds'] = 760
                row['status']['conditions'][0]['lastTransitionTime'] = (
                    dt.datetime.now(dt.timezone.utc) - dt.timedelta(minutes=11)).isoformat()
            pods[name] = rows
        hp = {name: hpa(role) for role, name in (('am', 'cma-agent-manager'), ('eg', 'cma-event-gateway'))}
        def get(kind, name):
            return objects[name] if kind == 'Deployment' else hp[name]
        def stable(obj, *args, **kwargs):
            return next(pods[name] for name, value in objects.items() if value is obj)
        with patch.object(g, 'get', side_effect=get), patch.object(checkpoints, 'stable', side_effect=stable):
            self.assertEqual(len(e.stable_pairs(release())[0]), 2)
            hp['cma-agent-manager']['spec']['metrics'][0]['resource']['target']['averageUtilization'] = 25
            with self.assertRaisesRegex(RuntimeError, 'both_original_policies'):
                e.stable_pairs(release())
            hp['cma-agent-manager'] = hpa()
            row = pods['cma-agent-manager'][0]
            row['spec']['terminationGracePeriodSeconds'] = 735
            with self.assertRaisesRegex(RuntimeError, 'actual_760_grace'):
                e.stable_pairs(release())
            row['spec']['terminationGracePeriodSeconds'] = 760
            row['status']['conditions'][0]['lastTransitionTime'] = g.now()
            with self.assertRaisesRegex(RuntimeError, 'ten_minutes_ready'):
                e.stable_pairs(release())

    def test_pod_metrics_freshness_window_and_coverage(self):
        _, pods = processes(deployment(), 1)
        sample = {'metadata': {'name': 'pod-0'}, 'timestamp': g.now(), 'window': '30s',
                  'containers': [{'name': 'api', 'usage': {'cpu': '100m'}}]}
        with patch.object(g, 'command', return_value={'items': [sample]}):
            e.fresh_metrics('cma-agent-manager', pods)
            for field, value in (('timestamp', '2020-01-01T00:00:00Z'), ('window', '120s'),
                                 ('containers', [])):
                changed = copy.deepcopy(sample); changed[field] = value
                with self.subTest(field=field), patch.object(g, 'command', return_value={'items': [changed]}), \
                     self.assertRaisesRegex(RuntimeError, 'S3b_'):
                    e.fresh_metrics('cma-agent-manager', pods)
        with patch.object(g, 'command', return_value={'items': []}), self.assertRaises(RuntimeError):
            e.fresh_metrics('cma-agent-manager', pods)

    def test_metric_change_after_approval_never_writes(self):
        with tempfile.TemporaryDirectory() as root:
            obj, op = lower(root); path = Path(root) / 'lower.json'
            _, after = prepared(path, operation=op, original=obj)
            def policy(*args):
                if args:
                    obj['status']['currentMetrics'][0]['resource']['current']['averageUtilization'] = 29
            with patch.object(g, 'get', return_value=obj), patch.object(c, 'write_window'), \
                 patch.object(c, 'policy', side_effect=policy), \
                 patch.object(e, 'stable_pairs', return_value=(op['deployment_bindings'], {'am': []})), \
                 patch.object(e, 'fresh_metrics'), patch.object(g, 'command', return_value=after) as command, \
                 self.assertRaisesRegex(RuntimeError, 'metric_changed_replan'):
                c.apply(path, 'test-only-approval')
            self.assertEqual(command.call_count, 1)
            self.assertFalse(path.with_suffix('.result.json').exists())

    def test_missing_HPA_metric_or_condition_fails_closed(self):
        for missing in ('currentMetrics', 'conditions', 'observedGeneration'):
            obj = hpa(); del obj['status'][missing]
            with self.subTest(missing=missing), self.assertRaisesRegex(RuntimeError, 'S3b_'):
                e.utilization(obj)
