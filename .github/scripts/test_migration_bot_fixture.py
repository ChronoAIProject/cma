#!/usr/bin/env python3
"""Reject false compatibility receipts when retained Bot state or effects change."""
from copy import deepcopy
import io
import json
from pathlib import Path
from types import SimpleNamespace
import tempfile
import unittest
from unittest.mock import Mock, patch

from migration_bot_fixture import BotFixture, STATES


def records():
    result = []
    for index, state in enumerate(STATES):
        identifier = 'bot_' + f'{index:032x}'
        current = {'bot_id': identifier, 'state': state, 'operation': None,
                   'profile': {'id': 'profile', 'revision': 1}, 'channel': {'version': 2}}
        result.append({'state': state, 'route': '/api/v1/bots/' + identifier,
                       'body': {'name': state}, 'key': 'retained-' + state, 'current': current})
    return result


class BotReaderTests(unittest.TestCase):
    def fixture(self, directory, saved, corrupt=None):
        def request(method, route, body=None, key=None, status=200):
            row = next((r for r in saved if r['key'] == key or r['route'] == route), None)
            if route == '/api/v1/bots?limit=200':
                page = {'items': [deepcopy(r['current']) for r in saved if r['state'] != 'deleted'],
                        'next_after': None}
                if corrupt == 'pagination':
                    page['next_after'] = 'missing-next-page'
                if corrupt == 'listing':
                    page['items'] = []
                return page
            if method == 'POST' and body != row['body']:
                self.assertEqual(status, 409)
                return {'code': 'wrong' if corrupt == 'conflict' else 'bot_conflict'}
            if method == 'GET' and row['state'] == 'deleted':
                self.assertEqual(status, 404)
                return {'code': 'wrong' if corrupt == 'tombstone' else 'bot_not_found'}
            actual = deepcopy(row['current'])
            if corrupt == method:
                actual['profile']['revision'] += 1
            return actual
        return BotFixture(SimpleNamespace(request=request, output=directory))

    def test_both_reader_directions_preserve_all_four_saved_states(self):
        with tempfile.TemporaryDirectory() as directory:
            root, saved = Path(directory), records()
            counters = {'channels': 2, 'keys': 2, 'effects': {}, 'key_effects': {}}
            for stage, writer in [('new-read-old-write', 'old-write'), ('old-read-new-write', 'new-write')]:
                (root / (writer + '-bots.private.json')).write_text(json.dumps(saved))
                fixture = self.fixture(root, saved)
                with patch.object(fixture, 'dependencies', return_value=counters):
                    receipt = fixture.exercise(stage, [])
                self.assertEqual(receipt['collections'], ['bots'])
                self.assertEqual(receipt['samples'], 4)
                self.assertTrue(all(receipt['checks'].values()))

    def test_missing_nested_data_replay_or_list_never_passes(self):
        with tempfile.TemporaryDirectory() as directory:
            for corrupt in ['GET', 'POST', 'pagination', 'listing', 'conflict', 'tombstone']:
                saved = records()
                fixture = self.fixture(Path(directory), saved, corrupt)
                selected = saved[-1] if corrupt == 'tombstone' else saved[0]
                with self.subTest(corrupt=corrupt), self.assertRaises(RuntimeError):
                    fixture.verify(selected)

    def test_reader_that_repeats_an_effect_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root, saved = Path(directory), records()
            (root / 'old-write-bots.private.json').write_text(json.dumps(saved))
            fixture = self.fixture(root, saved)
            with patch.object(fixture, 'dependencies', side_effect=[{'effects': 0}, {'effects': 1}]):
                with self.assertRaisesRegex(RuntimeError, 'repeated an external effect'):
                    fixture.exercise('new-read-old-write', [])

    def test_missing_shape_or_unsupported_stage_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'old-write-bots.private.json').write_text(json.dumps(records()[:3]))
            fixture = self.fixture(root, records())
            with patch.object(fixture, 'dependencies', return_value={}):
                for stage in ['new-read-old-write', 'unknown']:
                    with self.subTest(stage=stage), self.assertRaises(RuntimeError):
                        fixture.exercise(stage, [])

    def test_settle_requires_completed_work_and_paces_polls(self):
        fixture = BotFixture(SimpleNamespace(request=Mock(side_effect=[
            {'state': 'active', 'operation': {'step': 'admitted'}},
            {'state': 'active', 'operation': None}]), output=Path('.')))
        with patch('migration_bot_fixture.time.sleep') as sleep:
            self.assertEqual(fixture.settle('/bot', 'active')['operation'], None)
            sleep.assert_called_once_with(2)
        fixture.request = Mock(return_value={'state': 'creating'})
        with patch('migration_bot_fixture.time.monotonic', side_effect=[0, 200]):
            with self.assertRaisesRegex(RuntimeError, 'failed to settle'):
                fixture.settle('/bot', 'active')


class BotWriterTests(unittest.TestCase):
    def backend(self, directory):
        rows, creates = {}, {}
        stats = {'channels': 0, 'keys': 0, 'effects': {'create': 0, 'delete': 0}}

        def request(method, route, body=None, key=None, status=200):
            if route == '/api/v1/bots' and method == 'POST':
                if key in creates:
                    identifier, original = creates[key]
                    return deepcopy(rows[identifier]) if original == body else {'code': 'bot_conflict'}
                identifier = 'bot_' + f'{len(rows):032x}'
                failure = body['connection_ref'].endswith('-failed')
                rows[identifier] = {k: body[k] for k in ['name', 'connection_ref', 'profile',
                                                      'sandbox_profile', 'workspace_profile']}
                rows[identifier].update(bot_id=identifier, state='failed' if failure else 'active',
                                        operation=None, channel=None if failure else {'version': 1},
                                        credentials_expire_at=None if failure else 'future',
                                        failure='fixture_refusal' if failure else None)
                creates[key] = identifier, deepcopy(body)
                stats['effects']['create'] += 1
                if failure:
                    stats['effects']['delete'] += 1
                else:
                    stats['channels'] += 1
                    stats['keys'] += 1
                return deepcopy(rows[identifier])
            if route == '/api/v1/bots?limit=200':
                return {'items': deepcopy([r for r in rows.values() if r['state'] != 'deleted']), 'next_after': None}
            parts = route.strip('/').split('/')
            row = rows[parts[3]]
            if method == 'POST' and parts[-1] == 'test':
                row['last_test'] = {'first_text': 'synthetic gateway response', 'tested_at': 'retained'}
                return deepcopy(row['last_test'])
            if method == 'POST' and parts[-1] == 'disable':
                row['state'] = 'disabled'
            if method == 'POST' and parts[-1] == 'rotate-keys':
                self.assertEqual(row['state'], 'disabled')
                row['channel']['version'] += 1
            if method == 'DELETE':
                row.update(state='deleted', channel=None, credentials_expire_at=None)
                stats['effects']['delete'] += 1
                stats['channels'] -= 1
                stats['keys'] -= 1
            if method == 'GET' and row['state'] == 'deleted':
                return {'code': 'bot_not_found'}
            return deepcopy(row)
        fixture = BotFixture(SimpleNamespace(request=request, output=directory))
        fixture.dependencies = lambda: deepcopy(stats)
        return fixture

    def test_writers_save_replayable_records_and_require_resource_cleanup(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture = self.backend(root)
            profiles = [{'kind': k, 'created': {'id': k + '-profile'}, 'revision': {'revision': 1}}
                        for k in ['agent', 'workspace', 'sandbox']]
            for stage in ['old-write', 'new-read-old-write', 'new-write', 'old-read-new-write']:
                self.assertEqual(fixture.exercise(stage, profiles)['samples'], 4)
            self.assertEqual(fixture.dependencies()['channels'], 4)
            self.assertEqual(len(json.loads((root / 'new-write-bots.private.json').read_text())), 4)
            with self.assertRaisesRegex(RuntimeError, 'profile set'):
                fixture.write('old-write', [])

    def test_dependency_observation_is_local_authenticated_and_complete(self):
        fixture = BotFixture(SimpleNamespace(request=None, output=Path('.'), admin_bearer='ephemeral'))
        counters = {'channels': 2, 'keys': 2, 'effects': {'create': 4, 'delete': 2, 'update': 2, 'test': 1},
                    'key_effects': {'mint': 5, 'revoke': 3}}
        with patch.dict('os.environ', {'MATRIX_IDENTITY_URL': 'http://127.0.0.1:12345'}):
            for result in [counters, {}, {**counters, 'keys': -1}]:
                with patch('migration_bot_fixture.urlopen', return_value=io.StringIO(json.dumps(result))) as call:
                    if result == counters:
                        self.assertEqual(fixture.dependencies(), counters)
                        self.assertIn('Authorization', dict(call.call_args.args[0].header_items()))
                    else:
                        with self.assertRaises(RuntimeError):
                            fixture.dependencies()
        with patch.dict('os.environ', {'MATRIX_IDENTITY_URL': 'https://foreign.example'}):
            with self.assertRaises(RuntimeError):
                fixture.dependencies()


if __name__ == '__main__':
    unittest.main()
