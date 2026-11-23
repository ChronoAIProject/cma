#!/usr/bin/env python3
"""Fault regressions for warm-pool evidence; not service compatibility proof."""
from copy import deepcopy
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from migration_warm_pool_fixture import ACTOR, ERRORS, FIELDS, ROUTE, RUNTIME, STAGES, WarmPoolFixture


class PoolAPI:
    admin_bearer = 'test-only-operator'

    def __init__(self, output):
        self.output, self.rows, self.commands, self.calls = output, {}, {}, []
        identifier = 'wp_' + hashlib.sha256(json.dumps(
            ['cma-system', 'system-default-v1-' + 'a' * 64], separators=(',', ':')).encode()).hexdigest()
        self.rows[identifier] = {'id': identifier, 'revision': 1, 'image': RUNTIME,
            'config_sha256': 'a' * 64, 'cpu_millis': 2000, 'memory_mib': 8192, 'target_replicas': 0,
            'ready_replicas': None, 'observed': False, 'status': 'unavailable',
            'resource_name': None, 'error': ERRORS[2]}

    def request(self, method, route, body=None, key=None, status=200,
                bearer=None, use_cookies=True):
        if bearer != self.admin_bearer or use_cookies:
            raise RuntimeError('operator authority required')
        self.calls.append((method, route, deepcopy(body), key))
        if method == 'GET':
            code, response = 200, {'items': list(self.rows.values()), 'default_image': RUNTIME,
                'default_cpu_millis': 2000, 'default_memory_mib': 8192,
                'default_config_sha256': 'a' * 64, 'available': True}
        elif method == 'POST':
            identifier = 'wp_' + hashlib.sha256(json.dumps(
                [ACTOR, key], separators=(',', ':')).encode()).hexdigest()
            digest = hashlib.sha256(json.dumps(body, sort_keys=True).encode()).hexdigest()
            if (identifier in self.rows and self.commands[key] != body
                or identifier not in self.rows and any(row['config_sha256'] == digest
                                                       for row in self.rows.values())):
                code, response = 409, {'code': 'operator_conflict'}
            else:
                if identifier not in self.rows:
                    self.rows[identifier] = {'id': identifier, 'revision': 1, 'image': RUNTIME,
                        'config_sha256': digest, **body, 'ready_replicas': None, 'observed': False,
                        'status': 'unavailable', 'resource_name': None, 'error': ERRORS[2]}
                    self.commands[key] = deepcopy(body)
                code, response = 202, self.rows[identifier]
        elif method == 'PUT':
            row = self.rows[route.rsplit('/', 1)[1]]
            if row['revision'] != body['expected_revision']:
                code, response = 409, {'code': 'operator_conflict'}
            else:
                row['revision'] += 1
                row['target_replicas'] = body['target_replicas']
                code, response = 202, row
        else:
            raise RuntimeError('unexpected route')
        if code != status:
            raise RuntimeError(f'HTTP {code}, expected {status}')
        return deepcopy(response)


class WarmPoolFixtureTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.output = Path(directory.name)
        self.api = PoolAPI(self.output)
        self.fixture = WarmPoolFixture(self.api)

    def writer(self):
        return self.fixture.exercise('old-write', [])

    def test_four_stages_keep_two_zero_capacity_plans_and_single_resize_per_writer(self):
        for stage in STAGES:
            result = self.fixture.exercise(stage, [])
            self.assertEqual(result['collections'], ['native_warm_pool_targets'])
            self.assertEqual(result['samples'], 1)
            self.assertTrue(all(result['checks'].values()))
        self.assertEqual(len(self.api.rows), 3)
        self.assertEqual(len(self.api.commands), 2)
        self.assertEqual({row['target_replicas'] for row in self.api.rows.values()}, {0})
        self.assertEqual(sorted(row['revision'] for row in self.api.rows.values()), [1, 2, 2])
        self.assertTrue(all(body.get('target_replicas', 0) == 0
                            for _, _, body, _ in self.api.calls if body))

    def test_every_durable_field_change_is_rejected(self):
        self.writer()
        identifier = next(key for key, row in self.api.rows.items() if row['revision'] == 2)
        original = deepcopy(self.api.rows[identifier])
        for field in FIELDS:
            self.api.rows[identifier] = {**original, field: 'corrupt'}
            with self.subTest(field=field), self.assertRaises(RuntimeError):
                self.fixture.exercise('new-read-old-write', [])

    def test_bound_or_running_provider_and_unknown_error_are_refused(self):
        self.writer()
        identifier = next(key for key, row in self.api.rows.items() if row['revision'] == 2)
        original = deepcopy(self.api.rows[identifier])
        for field, value in [('revision', True), ('target_replicas', True), ('target_replicas', 1),
                ('cpu_millis', True), ('memory_mib', 0), ('config_sha256', None),
                ('ready_replicas', 0), ('observed', True), ('resource_name', 'unexpected'),
                ('status', 'ready'), ('error', 'unexpected')]:
            self.api.rows[identifier] = {**original, field: value}
            with self.subTest(field=field), self.assertRaises(RuntimeError):
                self.fixture.exercise('new-read-old-write', [])

    def test_missing_duplicate_or_changed_inventory_is_rejected(self):
        self.writer()
        request = self.api.request
        for mode in ('missing', 'duplicate', 'default', 'extra'):
            def changed(method, route, **kwargs):
                value = request(method, route, **kwargs)
                if method == 'GET':
                    if mode == 'missing':
                        value['items'] = []
                    elif mode == 'duplicate':
                        value['items'] *= 2
                    elif mode == 'default':
                        value['default_image'] = 'other'
                    else:
                        value['items'].append({**value['items'][0], 'id': 'wp_' + 'f' * 64})
                return value
            with self.subTest(mode=mode), patch.object(self.api, 'request', side_effect=changed):
                with self.assertRaises(RuntimeError):
                    self.fixture.exercise('new-read-old-write', [])

    def test_refusals_must_have_the_right_code_and_no_effect(self):
        self.writer()
        request = self.api.request
        original = deepcopy(self.api.rows)
        for mode in ('wrong_code', 'mutation', 'duplicate'):
            self.api.rows = deepcopy(original)
            def changed(method, route, **kwargs):
                value = request(method, route, **kwargs)
                if kwargs.get('status') == 409:
                    if mode == 'wrong_code':
                        value['code'] = 'unrelated'
                    elif mode == 'mutation':
                        next(iter(self.api.rows.values()))['cpu_millis'] += 1
                    else:
                        row = deepcopy(next(iter(self.api.rows.values())))
                        row['id'] = 'wp_' + 'f' * 64
                        self.api.rows[row['id']] = row
                return value
            with self.subTest(mode=mode), patch.object(self.api, 'request', side_effect=changed):
                with self.assertRaises(RuntimeError):
                    self.fixture.exercise('new-read-old-write', [])

    def test_replay_cannot_return_old_revision_or_rewrite_original_plan(self):
        self.writer()
        request = self.api.request
        for field, value in [('revision', 1), ('cpu_millis', 1500)]:
            def changed(method, route, **kwargs):
                response = request(method, route, **kwargs)
                return {**response, field: value} if method == 'POST' and kwargs.get('status') == 202 else response
            with self.subTest(field=field), patch.object(self.api, 'request', side_effect=changed):
                with self.assertRaisesRegex(RuntimeError, 'command replay changed target'):
                    self.fixture.exercise('new-read-old-write', [])

    def test_reconciliation_wait_has_read_only_pacing_and_a_hard_bound(self):
        self.writer()
        identifier = next(key for key, row in self.api.rows.items() if row['revision'] == 2)
        self.api.rows[identifier]['error'] = None
        now = [0]
        sleeps = []
        def sleep(delay):
            sleeps.append(delay)
            now[0] += delay
        fixture = WarmPoolFixture(self.api, clock=lambda: now[0], sleep=sleep)
        self.api.calls.clear()
        with self.assertRaisesRegex(RuntimeError, 'did not finish'):
            fixture.wait_reconciliation(identifier)
        self.assertEqual(sleeps, [2] * 20)
        self.assertEqual(len(self.api.calls), 21)
        self.assertEqual({call[0] for call in self.api.calls}, {'GET'})

    def test_invalid_saved_provenance_and_unavailable_api_fail_without_receipt(self):
        with self.assertRaisesRegex(RuntimeError, 'unknown stage'):
            self.fixture.exercise('wrong', [])
        with patch.object(self.api, 'request', side_effect=RuntimeError('HTTP 503')):
            with self.assertRaisesRegex(RuntimeError, 'HTTP 503'):
                self.writer()
        self.assertFalse((self.output / 'old-write-warm-pool.private.json').exists())
        self.writer()
        path = self.output / 'old-write-warm-pool.private.json'
        original = json.loads(path.read_text())
        for change in ('writer', 'key', 'body', 'revision'):
            value = deepcopy(original)
            if change == 'writer':
                value['writer'] = 'new-write'
            elif change == 'key':
                value['key'] = 'different'
            elif change == 'body':
                value['body']['cpu_millis'] += 1
            else:
                value['current']['revision'] = 1
            path.write_text(json.dumps(value))
            with self.subTest(change=change), self.assertRaises(RuntimeError):
                self.fixture.exercise('new-read-old-write', [])

    def test_missing_operator_authority_and_wrong_created_identity_are_refused(self):
        self.api.admin_bearer = ''
        with self.assertRaisesRegex(RuntimeError, 'operator bearer'):
            self.writer()
        self.api.admin_bearer = 'test-only-operator'
        request = self.api.request
        def changed(method, route, **kwargs):
            value = request(method, route, **kwargs)
            return {**value, 'id': 'wp_' + 'f' * 64} if method == 'POST' else value
        with patch.object(self.api, 'request', side_effect=changed):
            with self.assertRaisesRegex(RuntimeError, 'original command identity'):
                self.writer()
        self.assertFalse((self.output / 'old-write-warm-pool.private.json').exists())

    def test_system_default_is_exact_and_retained_across_readers(self):
        self.writer()
        path = self.output / 'warm-pool-default.private.json'
        original = json.loads(path.read_text())
        for field in FIELDS:
            path.write_text(json.dumps({**original, field: 'changed'}))
            with self.subTest(field=field), self.assertRaisesRegex(RuntimeError, 'retained system default'):
                self.fixture.exercise('new-read-old-write', [])
        path.unlink()
        with self.assertRaisesRegex(RuntimeError, 'original system default missing'):
            self.fixture.exercise('new-read-old-write', [])


if __name__ == '__main__':
    unittest.main()
