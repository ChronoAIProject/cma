"""Offline EG-only topology and lifecycle regressions; external commands denied."""
import argparse
import copy
import datetime as dt
import json
from pathlib import Path
import tempfile
from unittest.mock import patch

import controller as c
import eg_hpa as e
import hpa_exercise as exercise
import operations as o
import rules as g
from test_controller import NoExternalCommands, deployment, image, release
from test_hpa_exercise import hpa
from test_quota import quota
from test_recovery import processes


def timestamp(seconds=0):
    return (dt.datetime.now(dt.timezone.utc) - dt.timedelta(seconds=seconds)).isoformat()


class EGOnlyTests(NoExternalCommands):
    def setUp(self):
        super().setUp()
        self.release = release()
        self.objects, self.sets, self.pods, self.metrics = {}, [], [], []
        for role, count in (('am', 1), ('eg', 2)):
            obj = deployment(count)
            obj['metadata']['uid'] = role + '-deployment'
            obj['spec']['selector']['matchLabels']['app.kubernetes.io/name'] = e.NAMES[role]
            spec = obj['spec']['template']['spec']
            spec['terminationGracePeriodSeconds'] = 735 if role == 'am' else 760
            app = spec['containers'][0]
            app.update(name=g.TARGETS[e.NAMES[role]], image=image(role),
                       resources={'requests': {'cpu': '4' if role == 'am' else '250m'}})
            flag = 'CHRONO_AGENT_MANAGER_REPLICAS_ENABLED' if role == 'am' else 'CMAEG_REPLICAS_ENABLED'
            app['env'] = [{'name': flag, 'value': 'false' if role == 'am' else 'true'}]
            rs, pods = processes(obj, count)
            rs['metadata'].update(uid=role + '-rs', ownerReferences=[
                {'uid': obj['metadata']['uid'], 'controller': True}])
            for index, pod in enumerate(pods):
                name = role + '-pod-' + str(index)
                pod['metadata'].update(name=name, uid=name, ownerReferences=[
                    {'uid': role + '-rs', 'controller': True}])
                pod['spec']['terminationGracePeriodSeconds'] = spec['terminationGracePeriodSeconds']
                pod['status']['containerStatuses'][0].update(name=app['name'], imageID=image(role))
                pod['status']['conditions'][0]['lastTransitionTime'] = timestamp(700)
                self.metrics.append({'metadata': {'name': name}, 'timestamp': timestamp(),
                                     'window': '30s', 'containers': [
                                         {'name': app['name'], 'usage': {'cpu': '100m'}}]})
            self.objects[('Deployment', e.NAMES[role])] = obj
            self.sets.append(rs)
            self.pods.extend(pods)
        self.objects[('ResourceQuota', 'sandbox-quota')] = quota(True)
        self.objects[('PodDisruptionBudget', e.NAMES['eg'])] = {
            'metadata': {'uid': 'eg-pdb', 'generation': 1},
            'spec': {'minAvailable': 1, 'selector': {'matchLabels': {
                'app.kubernetes.io/name': e.NAMES['eg']}}},
            'status': {'observedGeneration': 1, 'currentHealthy': 2, 'disruptionsAllowed': 1}}
        self.release['eg_only_hpa'] = {'s2': {
            'state': 'PASS', 'source': self.release['source'], 'images': self.release['images'],
            'evidence_sha256': 'd' * 64, 'started_at': timestamp(1900), 'completed_at': timestamp(10),
            'deployments': {r: g.binding(self.dep(r)) for r in e.NAMES}}, 'capacity': {
                'evidence_sha256': 'e' * 64, 'topology': 'am1-off-eg2-on', 'observed_at': timestamp(),
                'eg_processes': 12, 'am_processes': 1, 'mongo_headroom_percent': 30,
                'quota_cpu': dict(g.QUOTA_AFTER), 'other_consumers_included': True,
                'all_live_terminating_clients_included': True, 'node_placement_qualified': True,
                'provider_budgets_qualified': True, 'survivor_qualified': True,
                'early_drain_qualified': True, 'quota_qualified': True}}
        for target, side_effect in (('get', self.get), ('command', self.command)):
            mock = patch.object(g, target, side_effect=side_effect)
            mock.start()
            self.addCleanup(mock.stop)
        self.mutations = []

    def dep(self, role):
        return self.objects[('Deployment', e.NAMES[role])]

    def get(self, kind, name):
        return self.objects.get((kind, name))

    def command(self, args, body=None):
        if 'get' in args:
            if '--raw' in args:
                return {'items': self.metrics}
            return {'items': self.sets if 'replicasets' in args else self.pods}
        self.mutations.append((args, body))
        if 'create' in args:
            obj = json.loads(body)
            obj['metadata'].update(uid='created-eg', resourceVersion='20')
        elif 'patch' in args:
            kind = next(k for k, v in g.KINDS.items() if v[0] == args[args.index('patch') + 1])
            obj = copy.deepcopy(self.get(kind, args[args.index('patch') + 2]))
            for row in json.loads(body):
                node = obj
                path = row['path'].strip('/').split('/')
                for key in path[:-1]:
                    node = node[int(key)] if isinstance(node, list) else node[key]
                key = int(path[-1]) if isinstance(node, list) else path[-1]
                if row['op'] == 'test':
                    assert node[key] == row['value'], 'mock API CAS refused'
                else:
                    node[key] = row['value']
            obj['metadata']['resourceVersion'] = '21'
        else:
            raise AssertionError('unexpected command')
        if '--dry-run=server' not in args:
            self.objects[(obj['kind'], obj['metadata']['name'])] = obj
        return obj

    def build(self, action='create', created=None):
        return o.build('s3-eg-only-' + action, self.release, created)

    def receipt(self, root, action='create', obj=None, operation=None):
        obj = obj or hpa('eg')
        obj.update(kind='HorizontalPodAutoscaler')
        obj['metadata'].update(name=e.NAMES['eg'], namespace=g.NAMESPACE)
        self.objects[('HorizontalPodAutoscaler', e.NAMES['eg'])] = obj
        op = operation or self.build(action)
        path = Path(root) / (action + '.result.json')
        g.save(path, {'state': 'APPLIED', 'source': self.release['source'],
                      'operation': op, 'after': g.binding(obj)})
        return obj, path

    def plan(self, root, action='create', created=None):
        packet, out = Path(root) / 'release.json', Path(root) / (action + '.json')
        g.save(packet, self.release)
        c.plan(argparse.Namespace(action='s3-eg-only-' + action, release=str(packet), out=str(out),
                                  created=created, pause_plan=None, prepare_plan=None))
        return out

    def test_create_binds_AM_one_off_EG_two_on_and_exact_policy(self):
        op = self.build()
        c.checkpoint(None, op, self.release)
        self.assertEqual(op['manifest']['spec'], exercise.original_spec('eg'))
        self.assertEqual(set(op['topology_bindings']), {'am', 'eg', 'pdb'})
        self.assertEqual(self.dep('am')['spec']['replicas'], 1)
        self.assertEqual(self.dep('am')['spec']['template']['spec']['terminationGracePeriodSeconds'], 735)

    def test_wrong_or_missing_S2_source_image_duration_refuses(self):
        saved = copy.deepcopy(self.release['eg_only_hpa']['s2'])
        for key, value in [('state', 'UNKNOWN'), ('source', 'f' * 40), ('images', {}),
                           ('evidence_sha256', None), ('completed_at', timestamp(-60)),
                           ('started_at', timestamp(500)), ('deployments', {})]:
            self.release['eg_only_hpa']['s2'] = {**saved, key: value}
            with self.subTest(key=key), self.assertRaises(RuntimeError):
                self.build()
        del self.release['eg_only_hpa']
        with self.assertRaisesRegex(RuntimeError, 'S2_PASS'):
            self.build()

    def test_underbudget_lifetime_capacity_and_unqualified_signal_refuse(self):
        saved = copy.deepcopy(self.release['hpa']['eg'])
        for key, value in [('hard_lifetime_seconds', 120), ('hard_lifetime_seconds', 759),
                           ('hard_lifetime_seconds', float('inf')), ('handoff_p95_seconds', 90.01),
                           ('capacity_processes_per_service', 11), ('cpu_signal_qualified', False)]:
            self.release['hpa']['eg'] = {**saved, key: value}
            with self.subTest(key=key, value=value), self.assertRaises(RuntimeError):
                self.build()

    def test_topology_capacity_requires_fresh_complete_all_consumer_headroom(self):
        saved = copy.deepcopy(self.release['eg_only_hpa']['capacity'])
        cases = [('observed_at', timestamp(121)), ('observed_at', timestamp(-5)),
                 ('eg_processes', 11), ('am_processes', 0), ('mongo_headroom_percent', 29.9),
                 ('mongo_headroom_percent', float('nan')), ('quota_cpu', {}), ('topology', 'paired')]
        cases += [(k, False) for k, v in saved.items() if v is True]
        for key, value in cases:
            self.release['eg_only_hpa']['capacity'] = {**saved, key: value}
            with self.subTest(key=key), self.assertRaises(RuntimeError):
                self.build()

    def test_flags_must_be_explicit_literals_on_template_and_actual_pods(self):
        for role in e.NAMES:
            spec = self.dep(role)['spec']['template']['spec']
            pod_spec = next(p['spec'] for p in self.pods if p['metadata']['name'].startswith(role))
            for target in (spec, pod_spec):
                app = target['containers'][0]
                original = copy.deepcopy(app['env'])
                for bad in ([], [dict(original[0], value='true' if role == 'am' else 'false')],
                            original * 2, [{'name': original[0]['name'], 'valueFrom': {}}]):
                    app['env'] = bad
                    self.release['eg_only_hpa']['s2']['deployments'][role] = g.binding(self.dep(role))
                    with self.subTest(role=role, bad=bad), self.assertRaises(RuntimeError):
                        self.build()
                app['env'] = original
                self.release['eg_only_hpa']['s2']['deployments'][role] = g.binding(self.dep(role))

    def test_unhealthy_stale_AM_and_AM_HPA_refuse(self):
        obj = self.dep('am')
        for key, value in [('readyReplicas', 0), ('observedGeneration', 3), ('updatedReplicas', 0)]:
            saved = obj['status'][key]; obj['status'][key] = value
            with self.subTest(key=key), self.assertRaises(RuntimeError):
                self.build()
            obj['status'][key] = saved
        self.objects[('HorizontalPodAutoscaler', e.NAMES['am'])] = hpa()
        with self.assertRaisesRegex(RuntimeError, 'AM_HPA_prohibited'):
            self.build()

    def test_EG_placement_termination_image_grace_and_PDB_refuse(self):
        pod = self.pods[-1]
        cases = [(lambda: pod['spec'], 'nodeName', 'host-0'),
                 (lambda: pod['metadata'], 'deletionTimestamp', timestamp()),
                 (lambda: pod['spec'], 'terminationGracePeriodSeconds', 735),
                 (lambda: pod['spec']['containers'][0], 'image', image('eg', 'f')),
                 (lambda: self.get('PodDisruptionBudget', e.NAMES['eg'])['status'], 'disruptionsAllowed', 0)]
        for locate, key, value in cases:
            target = locate()
            original = copy.deepcopy(target)
            target[key] = value
            with self.subTest(key=key), self.assertRaises(RuntimeError):
                self.build()
            target.clear(); target.update(original)

    def test_create_requires_fresh_complete_metrics_and_raised_quota(self):
        op = self.build()
        for samples in ([], [{**self.metrics[-1], 'timestamp': timestamp(61)}]):
            with patch.object(g, 'command', return_value={'items': samples}), self.assertRaises(RuntimeError):
                c.checkpoint(None, op, self.release)
        self.objects[('ResourceQuota', 'sandbox-quota')] = quota()
        with self.assertRaisesRegex(RuntimeError, 'raised_quota'):
            c.checkpoint(None, op, self.release)

    def test_create_apply_without_HPA_generation_and_postapproval_recheck(self):
        with tempfile.TemporaryDirectory() as root:
            path = self.plan(root)
            with patch.object(c, 'policy'), patch.object(c, 'write_window'):
                c.apply(path, 'exact-once')
            result = g.load(path.with_suffix('.result.json'))
            self.assertEqual(result['state'], 'APPLIED')
            self.assertIsNone(result['after']['generation'])
            self.assertEqual(len(self.mutations), 3)  # plan dry-run, apply dry-run, one write
            self.assertEqual(sum('--dry-run=server' not in args for args, _ in self.mutations), 1)

    def test_AM_restart_after_approval_never_dispatches(self):
        with tempfile.TemporaryDirectory() as root:
            path = self.plan(root)
            def policy(*args):
                if args:
                    self.pods[0]['status']['containerStatuses'][0]['restartCount'] += 1
            with patch.object(c, 'policy', side_effect=policy), patch.object(c, 'write_window'), \
                 self.assertRaisesRegex(RuntimeError, 'topology_changed'):
                c.apply(path, 'exact-once')
            self.assertTrue(all('--dry-run=server' in args for args, _ in self.mutations))
            self.assertFalse(path.with_suffix('.result.json').exists())

    def test_AM_identity_or_flag_drift_after_plan_refuses(self):
        op = self.build()
        self.dep('am')['metadata']['uid'] = 'replacement'
        with self.assertRaisesRegex(RuntimeError, 'drift'):
            c.checkpoint(None, op, self.release)

    def test_create_admission_drift_refuses_before_approval_or_write(self):
        with tempfile.TemporaryDirectory() as root:
            path = self.plan(root)
            def admission(args, body=None):
                response = self.command(args, body)
                if 'create' in args:
                    response['spec']['maxReplicas'] = 4
                return response
            with patch.object(g, 'command', side_effect=admission), patch.object(c, 'policy') as policy, \
                 self.assertRaisesRegex(RuntimeError, 'admission_change'):
                c.apply(path, 'once')
            policy.assert_not_called()
            self.assertTrue(all('--dry-run=server' in args for args, _ in self.mutations))

    def test_create_postwrite_drift_is_UNKNOWN_with_no_retry(self):
        with tempfile.TemporaryDirectory() as root:
            path = self.plan(root)
            def admission(args, body=None):
                response = self.command(args, body)
                if 'create' in args and '--dry-run=server' not in args:
                    response['spec']['maxReplicas'] = 4
                return response
            with patch.object(g, 'command', side_effect=admission), patch.object(c, 'policy'), \
                 patch.object(c, 'write_window'), self.assertRaisesRegex(RuntimeError, 'uncertain'):
                c.apply(path, 'once')
            self.assertEqual(g.load(path.with_suffix('.result.json'))['state'], 'UNKNOWN')
            with patch.object(g, 'get') as reads, self.assertRaisesRegex(RuntimeError, 'previous_attempt'):
                c.apply(path, 'retry')
            reads.assert_not_called()

    def test_quota_raise_restore_reuses_two_CPU_leaves_and_requires_HPA_absence(self):
        before = quota()
        self.objects[('ResourceQuota', 'sandbox-quota')] = before
        op = self.build('quota-raise')
        c.checkpoint(before, op, self.release)
        after, _ = o.transformed(before, op)
        self.assertEqual(after['spec']['hard'], {**before['spec']['hard'], **g.QUOTA_AFTER})
        after['status']['hard'] = copy.deepcopy(after['spec']['hard'])
        self.objects[('ResourceQuota', 'sandbox-quota')] = after
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / 'quota.json'
            g.save(path, {'state': 'APPLIED', 'source': self.release['source'], 'operation': op,
                          'quota_before_spec': before['spec'], 'after': g.binding(after)})
            restore = self.build('quota-restore', path)
            c.checkpoint(after, restore, self.release)
            restored, _ = o.transformed(after, restore)
            self.assertEqual(restored['spec'], before['spec'])
            self.objects[('HorizontalPodAutoscaler', e.NAMES['eg'])] = hpa('eg')
            with self.assertRaisesRegex(RuntimeError, 'requires_no_HPAs'):
                c.checkpoint(after, restore, self.release)
            after['status']['used']['requests.cpu'] = '41'
            with self.assertRaisesRegex(RuntimeError, 'usage_above'):
                c.checkpoint(after, restore, self.release)

    def test_lower_restore_uses_only_target_leaf_and_owned_chain(self):
        with tempfile.TemporaryDirectory() as root:
            obj, prior = self.receipt(root)
            op = self.build('lower-target', prior)
            c.checkpoint(obj, op, self.release)
            _, payload, spec, _ = c.payload(obj, op, True)
            patch_rows = json.loads(payload)
            self.assertEqual(patch_rows[-1], {'op': 'replace', 'path': exercise.TARGET_PATH, 'value': 25})
            expected = exercise.original_spec('eg'); expected['metrics'][0]['resource']['target']['averageUtilization'] = 25
            self.assertEqual(spec, expected)
            obj['spec'] = spec; obj['metadata']['generation'] += 1
            _, lower = self.receipt(root, 'lower-target', obj, op)
            self.release.pop('eg_only_hpa')
            self.dep('am')['status']['readyReplicas'] = 0
            obj['status'] = {}
            restore = self.build('restore-target', lower)
            c.checkpoint(obj, restore, self.release)
            self.assertEqual(c.payload(obj, restore)[2], exercise.original_spec('eg'))

    def test_target_exercise_keeps_utilization_ready_age_and_generation_guards(self):
        with tempfile.TemporaryDirectory() as root:
            obj, prior = self.receipt(root)
            obj['status']['currentMetrics'][0]['resource']['current']['averageUtilization'] = 11
            with self.assertRaisesRegex(RuntimeError, 'at_least_12'):
                self.build('lower-target', prior)
            obj['status']['currentMetrics'][0]['resource']['current']['averageUtilization'] = 30
            self.pods[-1]['status']['conditions'][0]['lastTransitionTime'] = timestamp()
            with self.assertRaisesRegex(RuntimeError, 'ten_minutes'):
                self.build('lower-target', prior)

    def test_freeze_delete_survives_topology_and_metric_failure(self):
        with tempfile.TemporaryDirectory() as root:
            obj, prior = self.receipt(root)
            del obj['metadata']['generation']
            g.save(prior, {**g.load(prior), 'after': g.binding(obj)})
            self.release.pop('eg_only_hpa')
            self.dep('am')['status']['readyReplicas'] = 0
            self.metrics = []
            freeze = self.build('freeze', prior)
            c.checkpoint(obj, freeze, self.release)
            _, body, spec, _ = c.payload(obj, freeze)
            self.assertEqual([r['path'] for r in json.loads(body)[:3]],
                             ['/metadata/uid', '/metadata/resourceVersion', '/spec'])
            obj['spec'] = spec
            _, frozen = self.receipt(root, 'freeze', obj, freeze)
            delete = self.build('delete', frozen)
            c.checkpoint(obj, delete, self.release)
            _, body, _, _ = c.payload(obj, delete)
            self.assertEqual(json.loads(body)['preconditions'], {'uid': obj['metadata']['uid'], 'resourceVersion': '10'})

    def test_recovery_refuses_wrong_source_uid_spec_unknown_or_wrong_chain(self):
        with tempfile.TemporaryDirectory() as root:
            obj, prior = self.receipt(root)
            saved = g.load(prior)
            for key, value in [('source', 'f' * 40), ('state', 'UNKNOWN'), ('after', {}),
                               ('operation', {**saved['operation'], 'name': e.NAMES['am']})]:
                g.save(prior, {**saved, key: value})
                with self.subTest(key=key), self.assertRaises(RuntimeError):
                    self.build('freeze', prior)
            g.save(prior, saved)
            with self.assertRaisesRegex(RuntimeError, 'receipt_chain'):
                self.build('delete', prior)
            obj['spec']['maxReplicas'] = 4
            with self.assertRaisesRegex(RuntimeError, 'drift'):
                self.build('freeze', prior)

    def test_later_S1_stays_blocked_until_EG_HPA_deleted(self):
        self.objects[('HorizontalPodAutoscaler', e.NAMES['eg'])] = hpa('eg')
        with self.assertRaisesRegex(RuntimeError, 'remove_both_HPAs'):
            c.checkpoint(self.dep('am'), o.build('s1-pause', self.release), self.release)

    def test_paired_S3_and_S3b_still_require_AM_two(self):
        for op in (o.build('s3-eg-create', self.release), o.build('s3-quota-raise', self.release)):
            with self.assertRaisesRegex(RuntimeError, 'fully_converged'):
                c.checkpoint(self.get('ResourceQuota', 'sandbox-quota'), op, self.release)
        with self.assertRaisesRegex(RuntimeError, 'fully_converged'):
            exercise.stable_pairs(self.release)

    def test_scope_rejects_AM_target_and_unsupported_actions(self):
        op = self.build(); op['name'] = e.NAMES['am']
        with self.assertRaisesRegex(RuntimeError, 'operation_scope'):
            c.checkpoint(None, op, self.release)
        with self.assertRaisesRegex(RuntimeError, 'unknown_action'):
            self.build('am-create')

    def test_optional_HPA_generation_never_weakens_Deployment_CAS(self):
        obj = hpa('eg'); expected = copy.deepcopy(obj)
        for present in (True, False):
            if not present:
                obj['metadata'].pop('generation'); obj['status'].pop('observedGeneration')
            rows = g.patch_for(obj, expected, kind='HorizontalPodAutoscaler')
            self.assertEqual('/metadata/generation' in [r['path'] for r in rows], present)
            self.assertEqual(exercise.utilization(obj), 30)
        obj['status']['observedGeneration'] = 1
        with self.assertRaisesRegex(RuntimeError, 'generation_not_current'):
            exercise.utilization(obj)
        obj = self.dep('am'); obj['metadata'].pop('generation')
        with self.assertRaisesRegex(RuntimeError, 'valid_generation'):
            g.patch_for(obj, obj, kind='Deployment')
        with self.assertRaisesRegex(RuntimeError, 'Deployment_generation'):
            self.build()
