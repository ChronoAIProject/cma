"""The fixture must reject quota resets, replay changes and midnight ambiguity."""
from copy import deepcopy
from datetime import datetime, timezone
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch

from migration_visitor_fixture import VisitorFixture, utc_day
from test_migration_launch_fixture import MemoryAPI


class VisitorAPI(MemoryAPI):
    def __init__(self, output):
        super().__init__(output)
        self.settings['settings']['limits'] = {'visitor_max_launches_per_day': 5, 'other': 9}
        self.last_retry_after = 30
        self.refusals = 0

    def admin_request(self, method, route, body=None):
        if method == 'PATCH' and 'limits' in body:
            assert body['expected_revision'] == self.settings['revision']
            self.settings['revision'] += 1
            self.settings['settings']['limits'] = deepcopy(body['limits'])
            return deepcopy(self.settings)
        return super().admin_request(method, route, body)

    def request(self, method, route, body=None, key=None, status=200):
        if method == 'POST' and route.endswith('/launches') and key not in self.commands:
            limit = self.settings['settings']['limits']['visitor_max_launches_per_day']
            if len(self.launches) >= limit:
                assert status == 429
                self.refusals += 1
                return {'code': 'resource_quota', 'resource': 'visitor_launches', 'limit': limit,
                        'reset_at': datetime.fromtimestamp((utc_day() + 1) * 86400,
                                                          timezone.utc).isoformat()}
            assert status == 201
        return super().request(method, route, body, key, status)


class VisitorTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.api = VisitorAPI(Path(temp.name))
        self.fixture = VisitorFixture(self.api)
        self.fixture.author.prepare_profiles = Mock()
        self.profiles = [{'kind': kind, 'created': {'id': kind}, 'revision': {'revision': 1}}
                         for kind in ('agent', 'workspace', 'sandbox')]

    def first(self):
        self.fixture.exercise('old-write', self.profiles)
        return json.loads((self.api.output / 'old-write-visitor.private.json').read_text())

    def test_full_sequence_preserves_replays_at_exhausted_quota(self):
        for stage in ('old-write', 'new-read-old-write', 'new-write', 'old-read-new-write'):
            result = self.fixture.exercise(stage, self.profiles)
            self.assertTrue(all(result['checks'].values()))
        self.assertEqual(len(self.api.launches), 4)
        self.assertEqual(self.api.refusals, 4)
        self.assertEqual(self.api.settings['settings']['limits']['other'], 9)

    def test_retained_limit_reset_and_wrong_writer_fail(self):
        self.first()
        self.api.settings['settings']['limits']['visitor_max_launches_per_day'] = 5
        with self.assertRaisesRegex(RuntimeError, 'retained daily limit'):
            self.fixture.exercise('new-read-old-write', self.profiles)
        self.api.settings['settings']['limits']['visitor_max_launches_per_day'] = 2
        path = self.api.output / 'old-write-visitor.private.json'
        record = json.loads(path.read_text()); record['writer'] = 'new-write'
        path.write_text(json.dumps(record))
        with self.assertRaisesRegex(RuntimeError, 'writer identity'):
            self.fixture.exercise('new-read-old-write', self.profiles)

    def test_day_boundary_never_claims_quota_continuity(self):
        record = self.first()
        with patch('migration_visitor_fixture.utc_day', return_value=record['day'] + 1):
            with self.assertRaisesRegex(RuntimeError, 'UTC day'):
                self.fixture.verify(record, 2)
            with self.assertRaisesRegex(RuntimeError, 'UTC quota'):
                self.fixture.exercise('new-write', self.profiles)

    def test_changed_replay_or_launch_is_rejected(self):
        record = self.first()
        command = record['launches'][0]['command']
        self.api.commands[command['key']]['state'] = 'ready'
        with self.assertRaisesRegex(RuntimeError, 'replay changed'):
            self.fixture.verify(record, 2)
        self.api.commands[command['key']] = command['response']
        self.api.launches[record['launches'][0]['route']]['event_head'] += 1
        with self.assertRaisesRegex(RuntimeError, 'Launch changed'):
            self.fixture.verify(record, 2)

    def test_wrong_refusal_is_not_daily_quota_proof(self):
        record = self.first()
        original = self.fixture.request
        for field, value in [('code', 'rate_limited'), ('resource', 'other'), ('limit', 4),
                             ('reset_at', '2000-01-01T00:00:00Z')]:
            def altered(*args, **kwargs):
                result = original(*args, **kwargs)
                if kwargs.get('status') == 429: result[field] = value
                return result
            self.fixture.request = altered
            with self.subTest(field=field), self.assertRaises(RuntimeError):
                self.fixture.verify(record, 2)
        self.fixture.request = original
        self.api.last_retry_after = None
        with self.assertRaisesRegex(RuntimeError, 'Retry-After'):
            self.fixture.verify(record, 2)

    def test_unrelated_settings_change_and_unexpected_runtime_fail(self):
        original = self.api.admin_request
        def changed(method, route, body=None):
            result = original(method, route, body)
            if method == 'PATCH': self.api.settings['settings']['other'] = 'changed'
            return result
        self.api.admin_request = changed
        with self.assertRaisesRegex(RuntimeError, 'unrelated settings'):
            self.fixture.limit('old-write')
        self.api.admin_request = original
        record = self.first()
        self.api.native = [{'id': 'unexpected'}]
        with self.assertRaisesRegex(RuntimeError, 'native resources'):
            self.fixture.verify(record, 2)

    def test_unsupported_stage_missing_limit_and_writer_midnight_fail(self):
        with self.assertRaisesRegex(RuntimeError, 'unsupported stage'):
            self.fixture.exercise('other', self.profiles)
        with patch('migration_visitor_fixture.utc_day', side_effect=[100, 101]):
            with self.assertRaisesRegex(RuntimeError, 'writer crossed'):
                self.fixture.exercise('old-write', self.profiles)
        self.api.settings['settings']['limits']['visitor_max_launches_per_day'] = '2'
        with self.assertRaisesRegex(RuntimeError, 'limit missing'):
            self.fixture.limit('old-write')


if __name__ == '__main__':
    unittest.main()
