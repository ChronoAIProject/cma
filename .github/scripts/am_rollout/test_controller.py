import copy
import datetime as dt
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

SPEC = importlib.util.spec_from_file_location('am_rollout', Path(__file__).with_name('controller.py'))
c = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(c)
OLD = c.REPOSITORY + '@sha256:' + 'a' * 64
NEW = c.REPOSITORY + '@sha256:' + 'b' * 64
NOW = dt.datetime(2026, 11, 16, 21, 30, tzinfo=dt.timezone.utc).timestamp()
WINDOW = {'start_epoch': NOW - 10, 'end_epoch': NOW + 30}


def deployment():
    return {'apiVersion': 'apps/v1', 'kind': 'Deployment', 'metadata': {
        'name': c.DEPLOYMENT, 'namespace': c.NAMESPACE, 'uid': 'deployment-uid',
        'resourceVersion': '100', 'generation': 248,
        'annotations': {'deployment.kubernetes.io/revision': '233'}}, 'spec': {
        'replicas': 2, 'minReadySeconds': 30, 'strategy': {'type': 'RollingUpdate',
        'rollingUpdate': {'maxSurge': 1, 'maxUnavailable': 0}}, 'template': {
        'metadata': {'labels': {'app': 'am'}}, 'spec': {'terminationGracePeriodSeconds': 735,
        'containers': [{'name': 'api', 'image': OLD, 'env': [
            {'name': name, 'valueFrom': {'fieldRef': {'apiVersion': 'v1', 'fieldPath': field}}}
            for name, field in c.IDENTITY.items()] + [{'name': 'RUST_MIN_STACK', 'value': '16777216'}],
        'readinessProbe': {'periodSeconds': 10, 'failureThreshold': 3},
        'lifecycle': {'preStop': {'exec': {'command': ['/bin/sleep', '30']}}}}]}}},
        'status': {'replicas': 2, 'readyReplicas': 2, 'availableReplicas': 2,
                   'updatedReplicas': 2, 'observedGeneration': 248}}


def pods():
    return {'items': [{'metadata': {'uid': name}, 'status': {'phase': 'Running',
        'conditions': [{'type': 'Ready', 'status': 'True'}], 'containerStatuses': [{
        'name': 'api', 'ready': True, 'restartCount': 0, 'state': {'running': {'startedAt': '2026-01-01'}},
        'containerID': 'containerd://' + name, 'imageID': OLD}]}} for name in ('pod-a', 'pod-b')]}


class FakeIO:
    def __init__(self, base, hpa, namespace):
        self.base, self.hpa, self.namespace = base, hpa, namespace
        self.calls = []
        self.denied = False
        self.mutation_error = False
        self.dry_drift = False
        self.pod_state = pods()
        self.current = copy.deepcopy(base)
        self.did_patch = False

    def get(self, args):
        self.calls.append(('get', args[0]))
        return copy.deepcopy({'deployment': self.current, 'hpa': self.hpa, 'namespace': self.namespace,
            'pods': self.pod_state, 'endpointslices': {'items': [{'endpoints': [
                {'targetRef': {'uid': n}, 'conditions': {'ready': True}}
                for n in ('pod-a', 'pod-b')]}]}}[args[0]])

    def policy(self, approval, context):
        self.calls.append(('policy', context))
        return subprocess.CompletedProcess([], 1 if self.denied else 0,
             'denied' if self.denied else 'ALLOWED once via reviewed-release-controller', '')

    def k(self, args):
        dry = '--dry-run=server' in args
        self.calls.append(('dry-run' if dry else 'mutation', args))
        patch = c.read_json(args[args.index('--patch-file') + 1])
        assert len([p for p in patch if p['op'] != 'test']) == 1
        assert patch[-1]['path'] == '/spec/template/spec/containers/0/image'
        result = copy.deepcopy(self.base)
        c.container(result)[1]['image'] = NEW
        if dry and self.dry_drift:
            result['spec']['template']['spec']['terminationGracePeriodSeconds'] = 810
        if not dry:
            self.did_patch = True
            if self.mutation_error:
                raise subprocess.TimeoutExpired('fake-kubectl', 20)
            self.current = result
        return subprocess.CompletedProcess([], 0, json.dumps(result), '')


class RolloutTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='am-rollout-offline-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.base = deployment()
        self.hpa = {'metadata': {'uid': 'hpa-uid'}, 'spec': {'minReplicas': 2, 'maxReplicas': 2}}
        self.namespace = {'metadata': {'uid': 'namespace-uid'}}
        self.rehearsal = copy.deepcopy(self.base)
        c.container(self.rehearsal)[1]['image'] = NEW
        self.packet = {'schema': 'am-successor-v1', 'context': c.CONTEXT, 'namespace': c.NAMESPACE,
            'execution_enabled': True, 'write_window': WINDOW,
            'baseline': {'deployment': self.ref('baseline', self.base), 'hpa': self.ref('hpa', self.hpa),
                         'namespace': self.ref('namespace', self.namespace)},
            'candidate': {'image': NEW, 'source': 'c' * 40,
                          'publication': self.ref('publication', {'status': 'PASS', 'image': NEW,
                              'source': 'c' * 40, 'exact_config_and_layers_match': True})},
            'rehearsal': {'deployment': self.ref('rehearsal', self.rehearsal), 'exceptions': [],
                'receipt': self.ref('receipt', {'status': 'PASS', 'source': 'c' * 40, 'image': NEW,
                    'template_sha256': c.hash_json(self.rehearsal['spec']['template']),
                    'checks': dict.fromkeys(c.REHEARSAL_CHECKS, 'PASS')})}}
        self.io = FakeIO(self.base, self.hpa, self.namespace)
        self.lock = self.root / 'writer.lock'

    def ref(self, name, value):
        p = self.root / (name + '.json')
        c.write_json(p, value)
        return {'path': str(p), 'sha256': c.digest(p.read_bytes())}

    def execute(self, suffix='run'):
        sha = c.hash_json(self.packet)
        review = {'verdict': 'GO', 'packet_sha256': sha,
                  'controller_sha256': c.digest(Path(c.__file__).read_bytes())}
        guard = c.window_guard
        with mock.patch.object(c, 'LOCK', self.lock), mock.patch.object(c, 'window_guard',
                side_effect=lambda window, clock=None: guard(window, lambda: NOW)):
            c.execute(self.packet, sha, review, 'test-only', self.root / suffix, self.io)

    def test_offline_exact_template_pass(self):
        _, planned, diff = c.validate_packet(self.packet)
        self.assertEqual(diff, [])
        self.assertEqual(c.container(planned)[1]['image'], NEW)
        self.assertEqual(self.base, deployment())

    def test_policy_is_last_external_call_before_single_mutation(self):
        self.execute()
        names = [v[0] for v in self.io.calls]
        at = names.index('mutation')
        self.assertEqual(names[at - 1], 'policy')
        self.assertEqual(names.count('mutation'), 1)
        self.assertEqual(names.count('policy'), 1)

    def test_denied_policy_never_mutates(self):
        self.io.denied = True
        with self.assertRaisesRegex(c.Refused, 'policy_check_refused'):
            self.execute()
        self.assertFalse(self.io.did_patch)

    def test_no_policy_on_failed_live_preflight(self):
        self.io.current['spec']['template']['spec']['terminationGracePeriodSeconds'] = 810
        with self.assertRaises(c.Refused):
            self.execute()
        self.assertNotIn('policy', [x[0] for x in self.io.calls])
        self.assertFalse(self.io.did_patch)

    def test_ambiguous_mutation_never_replayed_even_new_output(self):
        self.io.mutation_error = True
        with self.assertRaises(subprocess.TimeoutExpired):
            self.execute()
        with self.assertRaisesRegex(c.Refused, 'no_replay'):
            self.execute('second-output')
        self.assertEqual(sum(v[0] == 'mutation' for v in self.io.calls), 1)
        self.assertEqual(sum(v[0] == 'policy' for v in self.io.calls), 1)

    def test_saved_allowed_receipt_is_not_an_authorization(self):
        c.write_json(self.root / 'POLICY-CHECK.json', {'status': 'ALLOWED once via reviewed-release-controller'})
        self.io.denied = True
        with self.assertRaises(c.Refused):
            self.execute()
        self.assertFalse(self.io.did_patch)

    def test_preparation_only_never_calls_external_tools(self):
        self.packet['execution_enabled'] = False
        with self.assertRaisesRegex(c.Refused, 'preparation_only'):
            self.execute()
        self.assertEqual(self.io.calls, [])

    def test_rehearsal_grace_probe_and_hook_mismatches_fail(self):
        for field in ('grace', 'probe', 'hook'):
            with self.subTest(field=field):
                other = copy.deepcopy(self.rehearsal)
                if field == 'grace': other['spec']['template']['spec']['terminationGracePeriodSeconds'] = 810
                if field == 'probe': c.container(other)[1]['readinessProbe']['periodSeconds'] = 5
                if field == 'hook': c.container(other)[1]['lifecycle']['preStop']['exec']['command'] = ['native-drain']
                with self.assertRaises(c.Refused):
                    c.template_gate(self.rehearsal, other, [])

    def test_rehearsal_identity_or_stack_cannot_be_omitted(self):
        for name in list(c.IDENTITY) + ['RUST_MIN_STACK']:
            with self.subTest(name=name):
                other = copy.deepcopy(self.rehearsal)
                c.container(other)[1]['env'] = [e for e in c.container(other)[1]['env'] if e['name'] != name]
                with self.assertRaises(c.Refused):
                    c.template_gate(self.rehearsal, other, [])

    def test_behavior_exception_is_forbidden(self):
        other = copy.deepcopy(self.rehearsal)
        other['spec']['template']['spec']['terminationGracePeriodSeconds'] = 810
        diff = c.differences(self.rehearsal['spec']['template'], other['spec']['template'])
        with self.assertRaisesRegex(c.Refused, 'cannot_be_excepted'):
            c.template_gate(self.rehearsal, other, [{**diff[0], 'reason': 'local'}])

    def test_exact_local_reference_exception_and_stale_exception(self):
        other = copy.deepcopy(self.rehearsal)
        other['spec']['template']['metadata']['labels']['app'] = 'local-am'
        diff = c.differences(self.rehearsal['spec']['template'], other['spec']['template'])
        rules = [{**diff[0], 'reason': 'local selector'}]
        self.assertEqual(c.template_gate(self.rehearsal, other, rules), diff)
        rules[0]['rehearsal'] = 'different'
        with self.assertRaises(c.Refused): c.template_gate(self.rehearsal, other, rules)

    def test_non_image_dry_run_delta_refused_before_policy(self):
        self.io.dry_drift = True
        with self.assertRaisesRegex(c.Refused, 'non_image_change'): self.execute()
        self.assertNotIn('policy', [x[0] for x in self.io.calls])

    def test_stale_revision_uid_or_generation_refused(self):
        for which in ('revision', 'uid', 'generation'):
            self.io.current = copy.deepcopy(self.base)
            if which == 'revision': self.io.current['metadata']['annotations']['deployment.kubernetes.io/revision'] = '234'
            else: self.io.current['metadata'][which] = 'changed'
            with self.subTest(which=which), self.assertRaises(c.Refused): self.execute(which)
        self.assertNotIn('policy', [x[0] for x in self.io.calls])

    def test_hpa_change_refused(self):
        self.io.hpa = copy.deepcopy(self.hpa)
        self.io.hpa['spec']['maxReplicas'] = 3
        with self.assertRaisesRegex(c.Refused, 'HPA_changed'): self.execute()
        self.assertFalse(self.io.did_patch)

    def test_restart_refused(self):
        self.io.pod_state['items'][0]['status']['containerStatuses'][0]['restartCount'] = 1
        with self.assertRaisesRegex(c.Refused, 'identity_unproved'): self.execute()
        self.assertFalse(self.io.did_patch)

    def test_evidence_changed_is_refused(self):
        p = Path(self.packet['rehearsal']['deployment']['path'])
        p.write_text(p.read_text() + '\n')
        with self.assertRaisesRegex(c.Refused, 'evidence_hash_changed'): c.validate_packet(self.packet)

    def test_historical_success_does_not_qualify_new_template(self):
        receipt = c.bound(self.packet['rehearsal']['receipt'])
        receipt['template_sha256'] = 'old-template'
        self.packet['rehearsal']['receipt'] = self.ref('receipt2', receipt)
        with self.assertRaisesRegex(c.Refused, 'rehearsal_not_passed'): c.validate_packet(self.packet)

    def test_regular_json_file_with_spaces_works(self):
        path = self.root / 'file with spaces.json'
        c.write_json(path, {'works': True})
        self.assertEqual(c.read_json(path), {'works': True})

    def test_fifo_symlink_and_dev_fd_are_refused_without_blocking(self):
        fifo = self.root / 'fifo'
        os.mkfifo(fifo)
        link = self.root / 'link'
        link.symlink_to(self.packet['baseline']['deployment']['path'])
        for path in (fifo, link):
            with self.assertRaises(c.Refused): c.read_json(path)
        with open(self.packet['baseline']['deployment']['path']) as source:
            with self.assertRaises(c.Refused): c.read_json('/dev/fd/' + str(source.fileno()))

    def test_real_competing_process_cannot_bypass_lock_with_LOCK_HELD(self):
        code = 'import fcntl,sys; f=open(sys.argv[1],"a+"); fcntl.flock(f,fcntl.LOCK_EX|fcntl.LOCK_NB)'
        with c.writer_lock(self.lock):
            result = subprocess.run([sys.executable, '-c', code, str(self.lock)],
                                    env=dict(os.environ, LOCK_HELD='1'), capture_output=True, timeout=5)
            self.assertNotEqual(result.returncode, 0)
        with c.writer_lock(self.lock): pass
        self.assertTrue(self.lock.exists())

    @unittest.skipUnless(Path('/usr/bin/lockf').exists(), 'macOS lockf interoperability')
    def test_existing_lockf_and_python_lock_are_mutually_exclusive(self):
        with c.writer_lock(self.lock):
            result = subprocess.run(['/usr/bin/lockf', '-kn', '-t', '0', str(self.lock), '/usr/bin/true'],
                                    capture_output=True, timeout=5)
            self.assertEqual(result.returncode, 75)
        script = 'import fcntl,sys; f=open(sys.argv[1],"a+"); fcntl.flock(f,fcntl.LOCK_EX|fcntl.LOCK_NB)'
        result = subprocess.run(['/usr/bin/lockf', '-kn', '-t', '0', str(self.lock),
                                 sys.executable, '-c', script, str(self.lock)], capture_output=True, timeout=5)
        self.assertNotEqual(result.returncode, 0)

    def test_one_lock_for_full_execution_ignores_env_LOCK_HELD(self):
        with mock.patch.dict(os.environ, {'LOCK_HELD': '1'}): self.execute()
        self.assertTrue(self.io.did_patch)
        with c.writer_lock(self.lock): pass

    def test_window_and_blackout(self):
        for instant in (NOW - 11, NOW + 31):
            with self.assertRaises(c.Refused): c.window_guard(WINDOW, lambda: instant)
        blackout = dt.datetime(2026, 11, 16, 18, 20, tzinfo=dt.timezone.utc).timestamp()
        with self.assertRaisesRegex(c.Refused, 'blackout'):
            c.window_guard({'start_epoch': blackout - 1, 'end_epoch': blackout + 1}, lambda: blackout)

    def test_policy_crossing_window_does_not_mutate(self):
        path = self.root / 'patch.json'; c.write_json(path, [])
        times = iter([NOW, NOW, NOW + 40])
        with self.assertRaisesRegex(c.Refused, 'outside_lead_write_window'):
            c.dispatch_once(self.io, 'test', 'context', path, self.root / 'journal.json', WINDOW, lambda: next(times))
        self.assertFalse(self.io.did_patch)


if __name__ == '__main__':
    unittest.main(verbosity=2)
