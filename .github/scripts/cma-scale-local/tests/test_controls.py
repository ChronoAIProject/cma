#!/usr/bin/env python3
"""Offline control tests: synthetic kubectl only; no network or real kubeconfig."""
import json
import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

BUNDLE = Path(__file__).resolve().parent.parent
MOCK_KUBECTL = r'''#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
args = sys.argv[1:]
with Path(os.environ['CMA_REVIEW_CALLS']).open('a') as f:
    f.write(json.dumps(args) + '\n')
state = json.loads(Path(os.environ['CMA_REVIEW_STATE']).read_text())
if args[4:6] == ['config', 'view']:
    print(state.get('server', 'https://127.0.0.1:16443'), end='')
    sys.exit(0)
cmd = args[6:]
if state.get('fail_on') and cmd[:len(state['fail_on'])] == state['fail_on']:
    print('synthetic diagnostic payload: do not persist raw stderr', file=sys.stderr)
    sys.exit(42)
def persist():
    Path(os.environ['CMA_REVIEW_STATE']).write_text(json.dumps(state))
def meta(name):
    return {'name': name, 'namespace': 'cma-scale', 'uid': 'review-' + name,
            'resourceVersion': '7', 'generation': 1}
if cmd[:2] == ['get', 'hpa']:
    if state.get('malformed_hpa'):
        print('invalid synthetic JSON')
    else:
        print(json.dumps({'items': state.get('hpas', [])}))
elif cmd[:2] == ['get', 'deployment']:
    name = cmd[2]
    print(json.dumps({'metadata': meta(name), 'spec': {
        'replicas': state['replicas'][name], 'template': {
            'metadata': {'annotations': {'cma.chronoai.dev/source-sha': '0' * 40}},
            'spec': {'containers': [{'image': 'synthetic-review:local'}]}}}}))
elif cmd[:2] == ['delete', 'hpa']:
    state['hpas'] = [h for h in state['hpas'] if h['metadata']['name'] != cmd[2]]
    persist()
elif cmd[:1] == ['scale']:
    state['replicas'][cmd[1].split('/')[1]] = int(cmd[2].split('=')[1])
    persist()
elif cmd[:2] == ['rollout', 'status']:
    print('synthetic rollout completed')
elif cmd[:2] == ['get', 'pods']:
    selector = next((a for a in cmd if a.startswith('app.kubernetes.io/name=')), '')
    name = selector.split('=', 1)[-1]
    count = state.get('replicas', {}).get(name, 0)
    pods = [{'metadata': meta(name + '-' + str(i)), 'spec': {'nodeName': 'review-node-' + str(i)},
             'status': {'conditions': [{'type': 'Ready', 'status': 'True'}],
                        'containerStatuses': [{'imageID': 'sha256:' + '0' * 64}]}}
            for i in range(count)]
    if count and state.get('terminating_reads', 0):
        pods[0]['metadata']['deletionTimestamp'] = '2026-01-01T00:00:00Z'
        state['terminating_reads'] -= 1
        persist()
    print(json.dumps({'items': pods}))
else:
    print('unsupported synthetic operation', file=sys.stderr)
    sys.exit(99)
'''


class Fixture(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='.control-tests-', dir=BUNDLE)
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        for name in ['scripts', 'private', 'bin', 'evidence']:
            (self.root / name).mkdir()
        for name in ['k', 'mode.sh']:
            shutil.copyfile(BUNDLE / 'scripts' / name, self.root / 'scripts' / name)
            (self.root / 'scripts' / name).chmod(0o700)
        (self.root / 'private/kubeconfig').write_text('synthetic fixture, not a kubeconfig\n')
        (self.root / 'bin/kubectl').write_text(MOCK_KUBECTL)
        (self.root / 'bin/kubectl').chmod(0o700)
        self.calls_path = self.root / 'calls.jsonl'
        self.state_path = self.root / 'state.json'
        self.save_state({})
        self.env = dict(os.environ)
        for key in ['BASH_ENV', 'ENV', 'KUBECONFIG']:
            self.env.pop(key, None)
        self.env.update(PATH=str(self.root / 'bin') + os.pathsep + os.environ['PATH'],
                        CMA_REVIEW_CALLS=str(self.calls_path),
                        CMA_REVIEW_STATE=str(self.state_path))

    def save_state(self, value):
        self.state_path.write_text(json.dumps(value))

    def calls(self):
        return [json.loads(line) for line in self.calls_path.read_text().splitlines()] if self.calls_path.exists() else []

    def run_control(self, name, *args):
        return subprocess.run([str(self.root / 'scripts' / name), *args], cwd=self.root,
                              env=self.env, text=True, capture_output=True, timeout=30)


class WrapperTests(Fixture):
    def test_destination_and_identity_overrides_rejected_before_kubectl(self):
        forms = [
            ['-nreview-other'], ['-shttps://review.invalid:443'], ['-A=true'],
            ['-n', 'review-other'], ['-n=review-other'], ['-s', 'https://review.invalid'],
            ['-s=https://review.invalid'], ['-A'], ['-A=false'], ['-wnreview-other'],
            ['-wA'], ['-vnreview-other'], ['-onreview-other'],
            ['--namespace', 'review-other'], ['--namespace=review-other'],
            ['--server', 'https://review.invalid'], ['--server=https://review.invalid'],
            ['--all-namespaces'], ['--all-namespaces=true'], ['--all-namespaces=false'],
            ['--context', 'review-other'], ['--context=review-other'],
            ['--kubeconfig', 'review-file'], ['--kubeconfig=review-file'],
            ['--cluster=review-other'], ['--user=review-other'], ['--token=synthetic'],
            ['--client-certificate=review-file'], ['--client-key=review-file'],
            ['--password=synthetic'], ['--certificate-authority=review-file'],
            ['--insecure-skip-tls-verify'], ['--tls-server-name=review.invalid'],
            ['--proxy-url=https://review.invalid'], ['--as=review-subject'],
            ['--as-group=review-group'], ['--as-uid=review-subject'],
            ['--raw=https://review.invalid'], ['--', '-nreview-other'],
        ]
        for form in forms:
            with self.subTest(form=form):
                result = self.run_control('k', 'get', 'pods', *form)
                self.assertEqual(result.returncode, 2)
                self.assertEqual(self.calls(), [])

    def test_unsupported_commands_rejected_before_kubectl(self):
        for args in [('exec', 'review-pod', '--', 'sh'), ('attach', 'review-pod'),
                     ('cp', 'review-file', 'review-pod:/tmp'), ('config', 'use-context', 'review')]:
            with self.subTest(args=args):
                self.assertEqual(self.run_control('k', *args).returncode, 2)
                self.assertEqual(self.calls(), [])

    def test_nonlocal_server_stops_after_precheck(self):
        self.save_state({'server': 'https://review.invalid:443'})
        self.assertEqual(self.run_control('k', 'get', 'pods').returncode, 2)
        self.assertEqual(len(self.calls()), 1)
        self.assertEqual(self.calls()[0][4:6], ['config', 'view'])

    def test_supported_output_and_selector_forms_keep_pinned_destination(self):
        for args in [('get', 'pods', '-o', 'json'),
                     ('get', 'pods', '-o=json', '-l=app=review'),
                     ('get', 'pods', '-l', 'app=review', '-o', 'json')]:
            with self.subTest(args=args):
                result = self.run_control('k', *args)
                self.assertEqual(result.returncode, 0, result.stderr)
                expected = ['--kubeconfig', str(self.root / 'private/kubeconfig'),
                            '--context', 'kind-cma-scale', '--namespace', 'cma-scale', *args]
                self.assertEqual(self.calls()[-1], expected)


class FixedModeTests(Fixture):
    def setUp(self):
        super().setUp()
        self.names = ['cma-agent-manager', 'cma-event-gateway']
        hpas = []
        for name, target, kind in [('review-am', self.names[0], 'Deployment'),
                                   ('review-eg', self.names[1], 'Deployment'),
                                   ('review-unrelated', 'unrelated', 'Deployment'),
                                   ('review-other-kind', self.names[0], 'StatefulSet')]:
            hpas.append({'apiVersion': 'autoscaling/v2', 'kind': 'HorizontalPodAutoscaler',
                'metadata': {'name': name, 'namespace': 'cma-scale', 'uid': 'review-' + name,
                             'resourceVersion': '5', 'generation': 1},
                'spec': {'minReplicas': 2, 'maxReplicas': 5,
                         'scaleTargetRef': {'apiVersion': 'apps/v1', 'kind': kind, 'name': target}}})
        foreign_group = json.loads(json.dumps(hpas[0]))
        foreign_group['metadata']['name'] = 'review-other-api'
        foreign_group['metadata']['uid'] = 'review-other-api-uid'
        foreign_group['spec']['scaleTargetRef']['apiVersion'] = 'review.invalid/v1'
        hpas.append(foreign_group)
        self.save_state({'hpas': hpas, 'replicas': dict.fromkeys(self.names, 2)})

    def change_state(self, **changes):
        state = json.loads(self.state_path.read_text())
        state.update(changes)
        self.save_state(state)

    def attempts(self):
        return sorted((self.root / 'evidence/mode-transitions').iterdir())

    def run_mode(self, mode):
        result = self.run_control('mode.sh', mode)
        report = json.loads(result.stdout)
        receipt = Path(report['receipt'])
        saved = json.loads((receipt / 'result.json').read_text())
        self.assertEqual(saved['exit_code'], result.returncode)
        self.assertEqual(saved['status'], report['status'])
        return result, receipt, saved

    def operations(self):
        return [a[6:] for a in self.calls() if a[4:6] != ['config', 'view']]

    def tree_bytes(self, path):
        return {str(f.relative_to(path)): f.read_bytes() for f in path.iterdir() if f.is_file()}

    def test_fixed_sequence_preserves_first_snapshot_and_unrelated_hpas(self):
        first, receipt1, saved = self.run_mode('fixed-1')
        self.assertEqual(first.returncode, 0, first.stderr)
        self.assertEqual(saved['status'], 'succeeded')
        first_bytes = self.tree_bytes(receipt1)
        hpas = json.loads((receipt1 / 'hpa-before.json').read_text())
        self.assertEqual([h['metadata']['name'] for h in hpas['items']], ['review-am', 'review-eg'])
        self.assertEqual(hpas['unrelated_count'], 3)
        intent = json.loads((receipt1 / 'intent.json').read_text())
        self.assertEqual((intent['context'], intent['namespace'], intent['mode']),
                         ('kind-cma-scale', 'cma-scale', 'fixed-1'))
        for name in ['k', 'mode.sh']:
            self.assertEqual(intent['control_sha256'][name], hashlib.sha256((self.root / 'scripts' / name).read_bytes()).hexdigest())
        second, receipt2, _ = self.run_mode('fixed-2')
        self.assertEqual(second.returncode, 0, second.stderr)
        self.assertNotEqual(receipt1, receipt2)
        self.assertEqual(self.tree_bytes(receipt1), first_bytes)
        self.assertEqual(json.loads((receipt2 / 'hpa-before.json').read_text())['items'], [])
        state = json.loads(self.state_path.read_text())
        self.assertEqual([h['metadata']['name'] for h in state['hpas']],
                         ['review-unrelated', 'review-other-kind', 'review-other-api'])
        self.assertEqual(state['replicas'], dict.fromkeys(self.names, 2))
        self.assertFalse((self.root / 'evidence/hpa-before-fixed.json').exists())

    def test_failed_snapshot_cannot_damage_prior_success(self):
        result, previous, _ = self.run_mode('fixed-1')
        self.assertEqual(result.returncode, 0)
        captured = self.tree_bytes(previous)
        start = len(self.operations())
        self.change_state(fail_on=['get', 'hpa'])
        result, failed, saved = self.run_mode('fixed-2')
        self.assertEqual(result.returncode, 42)
        self.assertEqual(saved['phase'], 'snapshot-hpa')
        self.assertEqual(self.tree_bytes(previous), captured)
        self.assertNotEqual(previous, failed)
        self.assertFalse((failed / 'hpa-before.json').exists())
        self.assertFalse(any(c[0] in ['delete', 'scale'] for c in self.operations()[start:]))

    def test_retry_preserves_failed_attempt_and_legacy_checkpoint(self):
        legacy = self.root / 'evidence/hpa-before-fixed.json'
        legacy.write_text('synthetic legacy checkpoint\n')
        self.change_state(fail_on=['scale', 'deployment/' + self.names[1]])
        result, failed, _ = self.run_mode('fixed-1')
        self.assertEqual(result.returncode, 42)
        captured = self.tree_bytes(failed)
        self.change_state(fail_on=[])
        result, retry, _ = self.run_mode('fixed-2')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotEqual(failed, retry)
        self.assertEqual(self.tree_bytes(failed), captured)
        self.assertEqual(legacy.read_text(), 'synthetic legacy checkpoint\n')

    def test_snapshot_json_failure_prevents_mutations(self):
        self.change_state(malformed_hpa=True)
        result, _, saved = self.run_mode('fixed-1')
        self.assertEqual(result.returncode, 1)
        self.assertEqual(saved['error_type'], 'JSONDecodeError')
        self.assertFalse(any(c[0] in ['delete', 'scale'] for c in self.operations()))

    def test_deployment_snapshot_failure_prevents_mutations(self):
        self.change_state(fail_on=['get', 'deployment', self.names[1]])
        result, receipt, saved = self.run_mode('fixed-1')
        self.assertEqual(result.returncode, 42)
        self.assertEqual(saved['phase'], 'snapshot-deployments')
        self.assertTrue((receipt / 'hpa-before.json').exists())
        self.assertFalse(any(c[0] in ['delete', 'scale'] for c in self.operations()))

    def test_each_hpa_deletion_failure_retains_checkpoint_and_partial_journal(self):
        for target in ['review-am', 'review-eg']:
            with self.subTest(target=target):
                # Restore only synthetic state for each independent attempt.
                self.change_state(fail_on=['delete', 'hpa', target])
                start = len(self.operations())
                result, receipt, saved = self.run_mode('fixed-1')
                self.assertEqual(result.returncode, 42)
                self.assertEqual(saved['phase'], 'delete-hpa:' + target)
                self.assertEqual(len(json.loads((receipt / 'hpa-before.json').read_text())['items']), 2)
                self.assertFalse(any(c[0] == 'scale' for c in self.operations()[start:]))
                events = [json.loads(l) for l in (receipt / 'events.jsonl').read_text().splitlines()]
                outcomes = [(e['phase'], e['exit_code']) for e in events if e['state'] == 'finished']
                self.assertIn(('delete-hpa:' + target, 42), outcomes)
                if target == 'review-eg':
                    self.assertIn(('delete-hpa:review-am', 0), outcomes)

    def test_each_scale_failure_is_recorded_without_rolling_back(self):
        for index, name in enumerate(self.names):
            with self.subTest(name=name):
                self.change_state(fail_on=['scale', 'deployment/' + name], replicas=dict.fromkeys(self.names, 2))
                start = len(self.operations())
                result, receipt, saved = self.run_mode('fixed-1')
                self.assertEqual(result.returncode, 42)
                self.assertEqual(saved['phase'], 'scale:' + name)
                ops = self.operations()[start:]
                self.assertEqual([c[1] for c in ops if c[0] == 'scale'], ['deployment/' + n for n in self.names[:index + 1]])
                self.assertFalse(any(c[0] == 'rollout' for c in ops))
                state = json.loads(self.state_path.read_text())
                self.assertEqual(state['replicas'][self.names[0]], 2 if index == 0 else 1)
                self.assertEqual(state['replicas'][self.names[1]], 2)
                events = (receipt / 'events.jsonl').read_text()
                self.assertNotIn('synthetic diagnostic payload', events)
                self.assertIn('stderr_sha256', events)

    def test_rollout_failure_is_not_reported_as_success(self):
        self.change_state(fail_on=['rollout', 'status', 'deployment/' + self.names[0]])
        result, receipt, saved = self.run_mode('fixed-1')
        self.assertEqual(result.returncode, 42)
        self.assertEqual(saved['phase'], 'rollout:' + self.names[0])
        self.assertFalse((receipt / 'pods-after.json').exists())

    def test_terminating_pods_must_disappear_before_success(self):
        self.change_state(terminating_reads=1)
        result, receipt, saved = self.run_mode('fixed-1')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(saved['status'], 'succeeded')
        self.assertEqual(sum(c[:2] == ['get', 'pods'] for c in self.operations()), 4)
        snapshot = json.loads((receipt / 'pods-after.json').read_text())
        self.assertTrue(all(not p['terminating'] for pods in snapshot['services'].values() for p in pods))


if __name__ == '__main__':
    unittest.main()
