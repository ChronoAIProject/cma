#!/usr/bin/env python3
"""Guard evidence against false PASS when a reader drops or mutates Trigger data."""
from copy import deepcopy
import json
from pathlib import Path
from types import SimpleNamespace
import tempfile
import unittest
from unittest.mock import Mock, patch

import migration_profile_fixture as profile_fixture
from migration_profile_fixture import RateLimited
from migration_trigger_fixture import TriggerFixture


def record(state='draft'):
    route = '/api/v1/triggers/trigger-one'
    observed = {'detail': {'id': 'trigger-one', 'availability': state, 'draft_version': 1,
                           'draft': {'presentation': {'title': 'Preserve me'}}},
                'revisions': [] if state == 'draft' else [{'digest': 'original', 'revision': 1}],
                'placements': [{'id': 'place-one', 'retired_at': '2026-11-17T00:00:00Z'}],
                'active_placements': []}
    if state != 'draft':
        observed['preview'] = {'title': 'Preserve me', 'availability': state}
    return {'route': route, 'public_route': None if state == 'draft' else route + '-public',
            'observed': observed, 'commands': [
                {'method': 'POST', 'route': '/api/v1/triggers', 'body': {'content': 'original'},
                 'key': 'retained-key', 'status': 201,
                 'response': {'id': 'trigger-one', 'availability': 'draft'}}]}


class TriggerReaderTests(unittest.TestCase):
    def make_fixture(self, directory, saved):
        observed = deepcopy(saved['observed'])
        def respond(method, route, body=None, key=None, status=200, use_cookies=True):
            if method == 'PUT':
                self.assertEqual(status, 409)
                self.assertEqual(body['draft_version'], 2)
                return {'code': 'draft_version_conflict'}
            if method == 'POST':
                self.assertEqual((route, key, status), ('/api/v1/triggers', 'retained-key', 201))
                self.assertEqual(body, {'content': 'original'})
                return deepcopy(saved['commands'][0]['response'])
            routes = {saved['route']: 'detail', saved['route'] + '/revisions': 'revisions',
                      saved['route'] + '/placements?include_retired=true': 'placements',
                      saved['route'] + '/placements': 'active_placements'}
            if saved['public_route']:
                routes[saved['public_route']] = 'preview'
                if route == saved['public_route']:
                    self.assertFalse(use_cookies)
            return deepcopy(observed[routes[route]])
        request = Mock(side_effect=respond)
        return TriggerFixture(SimpleNamespace(output=Path(directory), request=request)), observed

    def test_each_state_preserves_full_values_and_replays_exact_command(self):
        for state in ('draft', 'published', 'retired'):
            with self.subTest(state=state), tempfile.TemporaryDirectory() as directory:
                saved = record(state)
                fixture, _ = self.make_fixture(directory, saved)
                fixture.verify(saved)
                methods = [call.args[0] for call in fixture.request.call_args_list]
                self.assertIn('POST', methods)
                self.assertEqual('PUT' in methods, state == 'draft')

    def test_lost_or_changed_records_are_not_accepted(self):
        for field in ('detail', 'revisions', 'placements', 'active_placements', 'preview'):
            with self.subTest(field=field), tempfile.TemporaryDirectory() as directory:
                saved = record('published')
                fixture, observed = self.make_fixture(directory, saved)
                observed[field] = {'changed': True}
                with self.assertRaisesRegex(RuntimeError, 'retained.*changed'):
                    fixture.verify(saved)
                self.assertTrue(all(call.args[0] == 'GET' for call in fixture.request.call_args_list))

    def test_replay_must_return_original_response(self):
        with tempfile.TemporaryDirectory() as directory:
            saved = record()
            fixture, _ = self.make_fixture(directory, saved)
            responder = fixture.request.side_effect
            fixture.request.side_effect = lambda method, *args, **kw: (
                {'id': 'duplicate'} if method == 'POST' else responder(method, *args, **kw))
            with self.assertRaisesRegex(RuntimeError, 'replay changed'):
                fixture.verify(saved)

    def test_replay_side_effect_even_with_same_receipt_fails(self):
        with tempfile.TemporaryDirectory() as directory:
            saved = record()
            fixture, observed = self.make_fixture(directory, saved)
            responder = fixture.request.side_effect
            def mutate(method, *args, **kwargs):
                if method == 'POST':
                    observed['detail']['draft_version'] += 1
                return responder(method, *args, **kwargs)
            fixture.request.side_effect = mutate
            with self.assertRaisesRegex(RuntimeError, 'mutated retained data'):
                fixture.verify(saved)

    def test_stale_update_http_failure_propagates(self):
        with tempfile.TemporaryDirectory() as directory:
            saved = record()
            fixture, _ = self.make_fixture(directory, saved)
            responder = fixture.request.side_effect
            def mismatch(method, *args, **kwargs):
                if method == 'PUT':
                    raise RuntimeError('returned 200, expected 409')
                return responder(method, *args, **kwargs)
            fixture.request.side_effect = mismatch
            with self.assertRaisesRegex(RuntimeError, 'expected 409'):
                fixture.verify(saved)

    def test_missing_or_incomplete_writer_evidence_fails(self):
        with tempfile.TemporaryDirectory() as directory:
            fixture, _ = self.make_fixture(directory, record())
            with self.assertRaises(FileNotFoundError):
                fixture.exercise('old-read-new-write', [])
            Path(directory, 'new-write-triggers.private.json').write_text('[]')
            with self.assertRaisesRegex(RuntimeError, 'expected draft'):
                fixture.exercise('old-read-new-write', [])
            with self.assertRaisesRegex(RuntimeError, 'unsupported stage'):
                fixture.exercise('unknown', [])

    def test_both_directions_select_the_correct_writer(self):
        for stage, writer in [('new-read-old-write', 'old-write'), ('old-read-new-write', 'new-write')]:
            with self.subTest(stage=stage), tempfile.TemporaryDirectory() as directory:
                rows = [record(state) for state in ('draft', 'published', 'retired')]
                fixture, _ = self.make_fixture(directory, rows[0])
                fixture.verify = Mock()
                Path(directory, writer + '-triggers.private.json').write_text(json.dumps(rows))
                receipt = fixture.exercise(stage, [])
                self.assertEqual(fixture.verify.call_count, 3)
                self.assertEqual(receipt['samples'], 3)
                self.assertEqual(receipt['collections'], ['trigger_definitions',
                                 'trigger_definition_revisions', 'operation_claims'])

    def test_fixture_paces_post_requests_at_default_create_interval(self):
        fixture = profile_fixture.Fixture.__new__(profile_fixture.Fixture)
        fixture.last_create_request_at = 100.0
        with patch.object(profile_fixture.time, 'monotonic', side_effect=[103.0, 109.25]), \
                patch.object(profile_fixture.time, 'sleep') as sleep:
            fixture._pace_create()
        sleep.assert_called_once_with(3.25)
        self.assertEqual(fixture.last_create_request_at, 109.25)

    def test_trigger_rate_limit_is_still_a_fixture_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            trigger, _ = self.make_fixture(directory, record())
            trigger.request.side_effect = RateLimited('POST', '/api/v1/triggers', 1)
            with self.assertRaisesRegex(RuntimeError, 'returned 429'):
                trigger.command('POST', '/api/v1/triggers',
                                {'content': 'original'}, 201)


if __name__ == '__main__':
    unittest.main()
