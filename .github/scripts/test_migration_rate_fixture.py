#!/usr/bin/env python3
"""Fault contracts for cross-process persisted input-budget observations."""
from copy import deepcopy
import json
import os
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import migration_rate_fixture as rate

OLD = {'source': 'a' * 40, 'image': 'registry/api@sha256:' + '1' * 64}
NEW = {'source': 'b' * 40, 'image': 'registry/api@sha256:' + '2' * 64}
CURRENT = {'source': 'c' * 40, 'image': 'registry/api@sha256:' + '3' * 64}
RECOVERY = {'source': 'd' * 40, 'image': 'registry/api@sha256:' + '4' * 64}


def plan(current, recovery):
    return {'readers': [{'id': role, 'role': role, 'artifact': artifact}
                        for role, artifact in [('historical', OLD), ('current', current),
                                               ('candidate', NEW), ('recovery', recovery)]]}


PLAN = plan(NEW, OLD)


class BudgetAPI:
    """An HTTP double used only to check assertions, never artifact evidence."""
    def __init__(self, output, ledger, clock):
        self.output, self.ledger, self.clock = Path(output), ledger, clock
        self.requests, self.cookies = [], {'cma_session': 'ephemeral-test-cookie'}
        self.admin_bearer, self.last_retry_after = 'ephemeral-test-bearer', None
        self.browser = SimpleNamespace(finish=lambda stage: {'session_owner_preserved': True},
                                       observations=[])
        self.failure = None

    def request(self, method, route, body=None, key=None, status=200, bearer=None, use_cookies=True):
        if self.failure:
            raise RuntimeError(self.failure)
        assert method == 'POST' and route.endswith('/input') and key
        assert body == {'text': 'migration budget probe'}
        subject = 'admin' if bearer else 'owner'
        assert (bearer == self.admin_bearer and not use_cookies) if bearer else use_cookies
        now = self.clock[0]
        if self.ledger.get(subject, 0) > now:
            actual, code = 429, 'rate_limited'
            self.last_retry_after = int(self.ledger[subject] - now)
        else:
            actual, code = 404, 'agent_not_found'
            self.ledger[subject] = now + 60
            self.last_retry_after = None
        self.requests.append({'status': actual, 'response': {'code': code},
                              'request_id': 'request-' + str(now),
                              'started_at': now, 'finished_at': now})
        self.clock[0] += 6
        if actual != status:
            raise RuntimeError(f'HTTP {actual}, expected {status}')
        return {'code': code}


class RateFixtureTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.output, self.clock, self.ledger = Path(self.directory.name), [1000], {}

    def fixture(self, stage='old-write', uid=None, artifact=None, declared=PLAN):
        api = BudgetAPI(self.output, self.ledger, self.clock)
        role = rate.STAGE_ROLES[stage]
        current = next(row['artifact'] for row in declared['readers'] if row['id'] == role)
        fixture = rate.RateFixture(api, stage, artifact or current, uid or stage, declared,
                                   clock=lambda: self.clock[0])
        return fixture

    def saved(self):
        return self.output / 'old-write-rate.private.json'

    def test_all_four_stages_use_distinct_subject_buckets_and_new_processes(self):
        request_counts = []
        for stage in rate.STAGES:
            fixture = self.fixture(stage)
            result = fixture.exercise()
            self.assertTrue(all(result['checks'].values()))
            self.assertEqual(result['collections'], ['request_rate_limits'])
            request_counts.append(len(fixture.fixture.requests))
        self.assertEqual(request_counts, [2, 1, 2, 1])
        self.assertEqual(set(self.ledger), {'owner', 'admin'})
        self.assertNotIn('ephemeral-test', self.saved().read_text())

    def test_distinct_current_and_recovery_read_the_correct_writer_records(self):
        release = plan(CURRENT, RECOVERY)
        for stage in rate.STAGES:
            fixture = self.fixture(stage, declared=release)
            self.assertTrue(all(fixture.exercise()['checks'].values()))
        for stage, wrong in [('new-read-old-write', NEW), ('old-read-new-write', OLD)]:
            with self.subTest(stage=stage), self.assertRaisesRegex(RuntimeError, 'reader artifact'):
                self.fixture(stage, artifact=wrong, declared=release)

    def test_fresh_allowance_after_boot_is_failure(self):
        self.fixture().exercise()
        self.ledger.clear()
        with self.assertRaisesRegex(RuntimeError, 'HTTP 404'):
            self.fixture('new-read-old-write').exercise()
        self.assertFalse((self.output / 'rate-new-read-old-write.json').exists())

    def test_same_process_and_wrong_reader_artifact_are_rejected(self):
        self.fixture().exercise()
        with self.assertRaisesRegex(RuntimeError, 'process was not replaced'):
            self.fixture('new-read-old-write', uid='old-write').exercise()
        with self.assertRaisesRegex(RuntimeError, 'reader artifact'):
            self.fixture('new-read-old-write', artifact=OLD)

    def test_generic_route_404_does_not_count_as_domain_admission(self):
        fixture = self.fixture()
        request = fixture.fixture.request
        def absent(*args, **kwargs):
            request(*args, **kwargs)
            return {'code': 'not_found'}
        fixture.fixture.request = absent
        with self.assertRaisesRegex(RuntimeError, 'Agent domain'):
            fixture.exercise()
        self.assertFalse(self.saved().exists())

    def test_wrong_refusal_code_and_retry_after_are_rejected(self):
        self.fixture().exercise()
        for code, delay in [('wrong', 40), ('rate_limited', None), ('rate_limited', 0),
                            ('rate_limited', 61), ('rate_limited', True), ('rate_limited', 3)]:
            with self.subTest(code=code, delay=delay):
                self.clock[0] = 1012
                fixture = self.fixture('new-read-old-write')
                request = fixture.fixture.request
                def refuse(*args, **kwargs):
                    request(*args, **kwargs)
                    fixture.fixture.last_retry_after = delay
                    return {'code': code}
                fixture.fixture.request = refuse
                with self.assertRaises(RuntimeError):
                    fixture.exercise()

    def test_expired_window_and_reversed_clock_fail_before_reader_request(self):
        self.fixture().exercise()
        for now in (999, 1045, 1061):
            self.clock[0] = now
            fixture = self.fixture('new-read-old-write')
            with self.assertRaises(RuntimeError):
                fixture.exercise()
            self.assertEqual(fixture.fixture.requests, [])

    def test_reader_response_crossing_window_is_rejected(self):
        self.fixture().exercise()
        self.clock[0] = 1042
        with self.assertRaisesRegex(RuntimeError, 'unrefilled budget window'):
            self.fixture('new-read-old-write').exercise()

    def test_writer_provenance_and_receipt_corruption_are_rejected(self):
        self.fixture().exercise()
        original = json.loads(self.saved().read_text())
        alterations = [('schema', 'wrong'), ('writer', 'new-write'), ('role', 'admin'),
                       ('artifact', NEW), ('reader_uid', ''), ('route', '/missing/input')]
        for field, value in alterations:
            record = deepcopy(original)
            record[field] = value
            self.saved().write_text(json.dumps(record))
            with self.subTest(field=field), self.assertRaises(RuntimeError):
                self.fixture('new-read-old-write').exercise()
        for part, field, value in [('charged', 'code', 'not_found'),
                                   ('denied', 'status', 404), ('denied', 'retry_after', None),
                                   ('charged', 'request_id', ''), ('charged', 'finished_at', 999)]:
            record = deepcopy(original)
            record[part][field] = value
            self.saved().write_text(json.dumps(record))
            with self.subTest(part=part, field=field), self.assertRaises(RuntimeError):
                self.fixture('new-read-old-write').exercise()

    def test_missing_browser_admin_or_pod_identity_is_rejected(self):
        api = BudgetAPI(self.output, self.ledger, self.clock)
        for field, value, stage in [('browser', None, 'old-write'), ('cookies', {}, 'old-write'),
                                    ('admin_bearer', '', 'new-write')]:
            with patch.object(api, field, value), self.assertRaises(RuntimeError):
                rate.RateFixture(api, stage, NEW if stage == 'new-write' else OLD, 'uid', PLAN)
        with self.assertRaises(RuntimeError):
            rate.RateFixture(api, 'old-write', OLD, '', PLAN)
        with self.assertRaises(RuntimeError):
            rate.RateFixture(api, 'wrong', OLD, 'uid', PLAN)

    def test_network_failure_emits_no_success_and_no_retry(self):
        fixture = self.fixture()
        fixture.fixture.failure = 'network unavailable'
        with self.assertRaisesRegex(RuntimeError, 'network unavailable'):
            fixture.exercise()
        self.assertEqual(list(self.output.iterdir()), [])

    def test_manifest_adapter_is_scoped_and_requires_provider(self):
        objects = [{'kind': 'ConfigMap', 'metadata': {'name': 'api-env'},
                    'data': {'CHRONO_AUTH_MODE': 'provider', 'unrelated': 'preserved'}}]
        before = deepcopy(objects)
        actual = rate.manifests(objects)
        self.assertEqual(objects, before)
        expected = deepcopy(objects)
        expected[0]['data']['CHRONO_SERVER_RATE_INPUT_PER_MINUTE'] = '1'
        self.assertEqual(actual, expected)
        for bad in ([], objects + objects, actual):
            with self.assertRaises(RuntimeError):
                rate.manifests(bad)
        objects[0]['data']['CHRONO_AUTH_MODE'] = 'trusted'
        with self.assertRaises(RuntimeError):
            rate.manifests(objects)

    def test_main_emits_scoped_receipt_and_preserves_failure_requests(self):
        plan_path = self.output / 'plan.json'
        plan_path.write_text(json.dumps(PLAN))
        env = {'MATRIX_STAGE': 'old-write', 'MATRIX_API_URL': 'http://127.0.0.1:1234',
               'MATRIX_SOURCE': OLD['source'], 'MATRIX_IMAGE': OLD['image'],
               'MATRIX_PLAN': str(plan_path), 'MATRIX_READER_POD_UID': 'uid'}
        api = BudgetAPI(self.output, self.ledger, self.clock)
        with patch.dict(os.environ, env), patch.object(rate, 'Fixture', return_value=api), \
                patch.object(rate.time, 'time', return_value=1012), \
                patch.object(rate.RateFixture, 'window'):
            rate.main()
        receipt = json.loads((self.output / 'fixture-receipt.json').read_text())
        self.assertEqual(receipt['outcome'], 'PASS')
        self.assertIn('request_rate_limits', receipt['collections'])
        (self.output / 'fixture-receipt.json').unlink()
        api.failure = 'HTTP unavailable'
        with patch.dict(os.environ, env), patch.object(rate, 'Fixture', return_value=api):
            with self.assertRaisesRegex(RuntimeError, 'HTTP unavailable'):
                rate.main()
        self.assertFalse((self.output / 'fixture-receipt.json').exists())
        self.assertTrue((self.output / 'requests-old-write.private.json').exists())


if __name__ == '__main__':
    unittest.main()
