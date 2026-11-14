"""Offline correctness checks: these never start Docker, Mongo, or an API."""
import copy
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import allocator_pair as driver
import hierarchy_seed as fixture


class AllocatorQualificationTests(unittest.TestCase):
    def test_initial_final_swap_readback_preserves_units_and_unavailable(self):
        observer = driver.repeat.Observer({'container': 'owned'})
        with patch.object(observer, 'docker', return_value='32768\n0\n') as read:
            value = observer.swap_readback('VmSwap:\t16 kB\n')
        self.assertEqual(value['process_vm_swap_kib'], 16)
        self.assertEqual(value['cgroup_memory_swap_current_bytes'], 32768)
        self.assertEqual(value['cgroup_memory_swap_max_bytes'], 0)
        self.assertEqual(read.call_count, 1)
        with patch.object(observer, 'docker', return_value='UNAVAILABLE\nmax\n'):
            value = observer.swap_readback('Threads: 10\n')
        self.assertEqual(value['process_vm_swap_kib'], 'UNAVAILABLE')
        self.assertEqual(value['cgroup_memory_swap_current_bytes'], 'UNAVAILABLE')
        self.assertEqual(value['cgroup_memory_swap_max_bytes'], 'max')
        with patch.object(observer, 'docker', side_effect=RuntimeError('private')):
            value = observer.swap_readback('VmSwap: 0 kB\n')
        self.assertNotIn('private', json.dumps(value))
        self.assertEqual(value['cgroup_readback_failure_type'], 'RuntimeError')

    def test_fixture_refusal_retains_status_without_body_or_subject(self):
        error = driver.repeat.HttpResponseError(401, 37)
        with patch.object(fixture.repeat, 'get', side_effect=error):
            with self.assertRaises(driver.repeat.HttpResponseError) as caught:
                fixture.Fixture().check({'zig': {'origin': 'http://127.0.0.1:18981'}}, 'unused')
        details = caught.exception.safe_details
        self.assertEqual(details['http_status'], 401)
        self.assertEqual(details['body_bytes'], 37)
        self.assertEqual(details['route'], fixture.SANDBOX_ROUTE)
        self.assertNotIn(fixture.SUBJECT, json.dumps(details))

    def test_nearest_rank_is_not_interpolated(self):
        values = list(range(20, 0, -1))
        self.assertEqual(driver.nearest_rank(values, .5), 10)
        self.assertEqual(driver.nearest_rank(values, .95), 19)
        self.assertEqual(driver.nearest_rank([7], .95), 7)
        self.assertIsNone(driver.nearest_rank([], .95))

    def test_failed_request_keeps_denominator_and_prevents_pass(self):
        rows = [{'outcome': 'completed', 'duration_seconds': .1},
                {'outcome': 'failed', 'duration_seconds': 20}]
        value = driver.latency(rows)
        self.assertEqual(value['attempted_samples'], 2)
        self.assertEqual(value['failed_samples'], 1)
        self.assertEqual(value['p95_ms'], 100)
        variants = {k: {'complete': True, 'full_anonymous_fit': {'mib_per_minute': .1},
                        'counts': {'aggregate_latency': {'p50_ms': 1, 'p95_ms': 2}}}
                    for k in ('mimalloc', 'zig')}
        self.assertTrue(driver.gates(variants)['aggregate_p50_p95_within_10_percent'])
        variants['mimalloc']['complete'] = False
        self.assertFalse(driver.gates(variants)['aggregate_p50_p95_within_10_percent'])
        self.assertFalse(driver.gates(variants)['candidate_anonymous_under_1'])

    def test_thresholds_are_strict_for_memory_and_inclusive_for_latency(self):
        variants = {k: {'complete': True, 'full_anonymous_fit': {'mib_per_minute': 0},
                        'counts': {'aggregate_latency': {'p50_ms': 100, 'p95_ms': 100}}}
                    for k in ('mimalloc', 'zig')}
        variants['mimalloc']['counts']['aggregate_latency'] = {'p50_ms': 110, 'p95_ms': 110}
        self.assertTrue(driver.gates(variants)['aggregate_p50_p95_within_10_percent'])
        variants['mimalloc']['counts']['aggregate_latency']['p95_ms'] = 110.01
        self.assertFalse(driver.gates(variants)['aggregate_p50_p95_within_10_percent'])
        variants['mimalloc']['full_anonymous_fit']['mib_per_minute'] = 1
        self.assertFalse(driver.gates(variants)['candidate_anonymous_under_1'])

    def test_full_fixed_mix_retains_failed_response_and_actual_latency(self):
        clock = [0.0]
        calls = [0]

        class Stop:
            def wait(self, seconds):
                clock[0] += seconds
                return False

        def get(origin, route):
            clock[0] += .01
            calls[0] += 1
            if calls[0] == 4:
                raise RuntimeError('private error must not escape')
            return b'{}'

        with tempfile.TemporaryDirectory() as folder, patch.object(driver.time, 'monotonic', side_effect=lambda: clock[0]), \
                patch.object(driver.repeat, 'get', side_effect=get), patch.object(fixture, 'validate_body'):
            result = driver.request_loop({'origin': 'http://127.0.0.1:18980'}, 0, Path(folder), Stop())
            self.assertEqual((result['planned'], result['attempted'], result['completed'], result['failed']), (1200,1200,1199,1))
            self.assertEqual(result['not_attempted'], 0)
            self.assertTrue(all(v['attempted_samples'] == 240 for v in result['routes'].values()))
            self.assertAlmostEqual(result['aggregate_latency']['p50_ms'], 10)
            journal = (Path(folder)/'requests.jsonl').read_text()
            self.assertEqual(len(journal.splitlines()), 1200)
            self.assertNotIn('private error', journal)

    def test_fixture_body_requires_real_identity_and_hierarchy(self):
        sandbox = {'id': fixture.SANDBOX, 'status': 'failed', 'workspace_count': 1,
                   'worktree_count': 0, 'agent_count': 0, 'workspaces': [{'id': fixture.WORKSPACE}]}
        workspace = {'id': fixture.WORKSPACE, 'sandbox_id': fixture.SANDBOX,
                     'status': 'ready', 'worktrees': [], 'agents': []}
        fixture.validate_body(fixture.SANDBOX_ROUTE, json.dumps(sandbox))
        fixture.validate_body(fixture.WORKSPACE_ROUTE, json.dumps(workspace))
        for body in ({}, {'status':404}, dict(sandbox, id='other'), dict(sandbox, workspaces=[])):
            with self.subTest(body=body), self.assertRaises(ValueError):
                fixture.validate_body(fixture.SANDBOX_ROUTE, json.dumps(body))
        with self.assertRaises(ValueError):
            fixture.validate_body(fixture.WORKSPACE_ROUTE, json.dumps(dict(workspace, agents=[{}])))

    def test_image_binding_distinguishes_manifest_config_and_source(self):
        config = {'application_base':driver.BASE, 'allocator_commit':driver.PATCH,
                  'candidate':{'docker_image_id':'sha256:'+'a'*64,'manifest':'sha256:'+'a'*64,
                               'config':'sha256:'+'b'*64,'source':driver.CANDIDATE}}
        actual = {'id':config['candidate']['docker_image_id'],'source':driver.CANDIDATE,'architecture':'amd64',
                  'descriptor':{'digest':config['candidate']['manifest'],
                                'annotations':{'config.digest':config['candidate']['config']}}}
        with patch.object(driver.pair, 'docker', return_value=json.dumps(actual)):
            self.assertEqual(driver.candidate_image(config),actual)
            for field in ('docker_image_id','manifest','config','source'):
                changed=copy.deepcopy(config);changed['candidate'][field]='sha256:'+'c'*64 if field!='source' else 'd'*40
                with self.subTest(field=field), self.assertRaises(ValueError):
                    driver.candidate_image(changed)


if __name__ == '__main__':
    unittest.main()
