"""Admission policy fixture must expose corruption instead of accepting it."""
from copy import deepcopy
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from migration_admission_policy_fixture import AdmissionPolicyFixture, STAGES
from migration_browser_fixture import OWNER


class API:
    def __init__(self, output):
        self.output, self.rows, self.writes = output, {}, []
        self.admit = True
        self.fail_read = False

    def request(self, method, route, body=None, key=None, status=200):
        if method == 'POST':
            assert route == '/api/v1/agents' and status == 202 and key
            number = len(self.rows) + 1
            identifier = 'sbx_' + format(number, '032x')
            if self.admit:
                self.rows[identifier] = {'id': identifier, 'owner': OWNER,
                    'name': body['name'] + ' sandbox operation-' + str(number),
                    'profile': {'id': 'p', 'revision': 1},
                    'limits': {}, 'created_at': '2026-01-01T00:00:00Z',
                    'revision': 1, 'status': 'provisioning'}
            return {'id': 'operation-' + str(number), 'status': 'accepted'}
        if route.startswith('/api/v1/agent-lifecycle/'):
            return {'id': route.rsplit('/', 1)[1], 'status': 'running'}
        if self.fail_read and any(row.get('always_on') for row in self.rows.values()):
            raise RuntimeError('GET Sandbox returned 503: retained admission corrupt')
        if '?' in route:
            return {'items': deepcopy(list(self.rows.values())), 'next': None}
        return deepcopy(self.rows[route.rsplit('/', 1)[1]])

    def admin_request(self, method, route, body=None, status=200):
        assert method == 'POST' and route.endswith('/always-on')
        row = self.rows[route.split('/')[-2]]
        assert row['revision'] == body['expected_revision']
        row.update(always_on=body['always_on'], revision=row['revision']+1)
        self.writes.append(deepcopy(body))
        return deepcopy(row)


class PolicyFixtureTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.output = Path(directory.name)
        self.api = API(self.output)
        self.fixture = AdmissionPolicyFixture(self.api)

    def test_four_stages_mutate_only_new_writer_and_preserve_parent(self):
        for stage in STAGES:
            result = self.fixture.exercise(stage, [])
            self.assertEqual(result['samples'], 1)
        self.assertEqual(self.api.writes, [{'expected_revision': 1, 'always_on': True}])
        self.assertEqual(len(self.api.rows), 2)

    def test_old_reader_corruption_is_failure_with_writer_evidence_retained(self):
        self.fixture.exercise('new-write', [])
        self.api.fail_read = True
        with self.assertRaisesRegex(RuntimeError, '503'):
            self.fixture.exercise('old-read-new-write', [])
        self.assertTrue((self.output/'new-write-admission-policy.private.json').exists())

    def test_bad_identity_revision_policy_or_runtime_is_rejected(self):
        self.fixture.exercise('new-write', [])
        identifier = next(iter(self.api.rows))
        original = deepcopy(self.api.rows[identifier])
        for mutation in ({'owner': 'wrong'}, {'revision': 0}, {'always_on': False},
                         {'status': 'ready'}, {'runtime_version': 'runtime'},
                         {'profile': {}}, {'created_at': 'changed'}):
            self.api.rows[identifier] = {**original, **mutation}
            with self.subTest(mutation=mutation), self.assertRaises(RuntimeError):
                self.fixture.exercise('old-read-new-write', [])

    def test_missing_parent_has_bounded_two_second_polling(self):
        self.api.admit = False
        with patch('migration_admission_policy_fixture.time.sleep') as sleep:
            with self.assertRaisesRegex(RuntimeError, '60s'):
                self.fixture.exercise('old-write', [])
        self.assertEqual(sleep.call_count, 30)
        self.assertTrue(all(call.args == (2,) for call in sleep.call_args_list))

    def test_refuses_incomplete_inventory_and_bad_stage(self):
        with patch.object(self.api, 'request', return_value={'items': [], 'next': 'cursor'}):
            with self.assertRaisesRegex(RuntimeError, 'inventory'):
                self.fixture.exercise('old-write', [])
        with self.assertRaisesRegex(RuntimeError, 'unknown stage'):
            self.fixture.exercise('unknown', [])

    def test_rejects_wrong_writer_provenance(self):
        self.fixture.exercise('new-write', [])
        path = self.output/'new-write-admission-policy.private.json'
        record = json.loads(path.read_text())
        record['writer'] = 'old-write'
        path.write_text(json.dumps(record))
        with self.assertRaisesRegex(RuntimeError, 'provenance'):
            self.fixture.exercise('old-read-new-write', [])

    def test_rejects_unrelated_parent_and_preexisting_policy(self):
        request = self.api.request
        for change in ({'name': 'unrelated'}, {'always_on': False}):
            self.api.rows.clear()
            def changed(method, route, *args, **kwargs):
                value = request(method, route, *args, **kwargs)
                return {**value, **change} if route.startswith('/api/v1/sandboxes/sbx_') else value
            with self.subTest(change=change), patch.object(self.api, 'request', side_effect=changed):
                with self.assertRaises(RuntimeError):
                    self.fixture.exercise('new-write', [])


if __name__ == '__main__':
    unittest.main()
