#!/usr/bin/env python3
"""Exercise retained report identities and legitimate worker transitions."""
from copy import deepcopy
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

from migration_usage_fixture import COLLECTIONS, ROUTE, UsageFixture


NOW = 1_200_000
REPORT_ID = 'umr_' + 'a' * 32


def report(status='pending', *, stage='old-write', at=NOW):
    query = {'from': NOW - 4200, 'to': NOW - 600, 'step_seconds': 60,
             'filters': {'search': 'matrix-' + stage + '-' + 'b' * 32}}
    return {'schema_version': 1, 'report_id': REPORT_ID, 'status': status,
            'report_complete': status == 'ready',
            'scope': {'principal': 'owner', 'admin': True}, 'query': query,
            'effective_range': {'from': query['from'], 'to': query['to']},
            'as_of': at, 'measurement_cutoff': at - 120, 'expires_at': at + 600,
            'summary': {} if status == 'ready' else None,
            'poll_url': ROUTE + '/' + REPORT_ID}


def record(observed):
    return {'route': ROUTE, 'report_id': observed['report_id'],
            'body': observed['query'], 'observed': observed}


def endpoint_shapes():
    return [{'items': [], 'report_id': REPORT_ID},
            {'lines': [], 'report_id': REPORT_ID},
            {'items': [], 'report_id': REPORT_ID}]


class UsageFixtureTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.output = Path(directory.name)
        self.uuid = patch('migration_usage_fixture.uuid.uuid4',
                          return_value=SimpleNamespace(hex='b' * 32))
        self.uuid.start()
        self.addCleanup(self.uuid.stop)

    def fixture(self, responses, *, clock=None):
        self.request = Mock(side_effect=responses)
        self.pause = Mock()
        return UsageFixture(SimpleNamespace(output=self.output, admin_request=self.request),
                            clock=clock or Mock(return_value=NOW), sleep=self.pause)

    def test_writer_uses_explicit_unique_query_and_checks_same_minute_coalescing(self):
        for stage in ('old-write', 'new-write'):
            with self.subTest(stage=stage):
                response = report(stage=stage)
                value = self.fixture([response, response, response])
                observed = value.write(stage)
                self.assertEqual(observed['report_id'], REPORT_ID)
                self.assertNotIn('ready_shapes', observed)
                self.assertEqual(observed['body'], response['query'])
                self.assertEqual(observed['coalescing_window'],
                                 {'started_at': NOW, 'finished_at': NOW})
                self.assertEqual(self.request.call_count, 3)
                first, second, third = self.request.call_args_list
                self.assertEqual(first, second)
                self.assertEqual(first.args, ('POST', ROUTE))
                self.assertEqual(first.kwargs['status'], (200, 202))
                self.assertEqual(third.args, ('GET', ROUTE + '/' + REPORT_ID))

    def test_writer_waits_before_admission_near_a_minute_boundary(self):
        response = report()
        # The next minute's range is deliberately supplied by the API fixture.
        response['query']['from'] += 60
        response['query']['to'] += 60
        response['effective_range'] = {key: response['query'][key] for key in ('from', 'to')}
        value = self.fixture([response] * 3,
                             clock=Mock(side_effect=[NOW + 58, NOW + 61, NOW + 68]))
        value.write('old-write')
        self.pause.assert_called_once_with(3)

    def test_crossed_minute_is_an_unmet_fixture_precondition_without_retry(self):
        value = self.fixture([report(), {**report(), 'report_id': 'umr_' + 'c' * 32}],
                             clock=Mock(side_effect=[NOW, NOW, NOW + 60]))
        with self.assertRaisesRegex(RuntimeError, 'minute precondition missed'):
            value.write('old-write')
        self.assertEqual(self.request.call_count, 2)
        self.assertFalse(list(self.output.iterdir()))

    def test_same_minute_changed_id_or_query_cannot_pass(self):
        for changed in ({**report(), 'report_id': 'umr_' + 'c' * 32},
                        {**report(), 'scope': {'admin': True, 'principal': 'different'}},
                        {**report(), 'query': {**report()['query'], 'step_seconds': 120}}):
            value = self.fixture([report(), changed])
            with self.subTest(changed=changed), self.assertRaisesRegex(
                    RuntimeError, 'report (ID|field) changed'):
                value.write('old-write')
            self.assertEqual(self.request.call_count, 2)

    def test_admission_must_retain_the_requested_range_and_writer_nonce(self):
        for field, changed in [('step_seconds', 120), ('filters', {'search': 'different'})]:
            value = self.fixture([{**report(), 'query': {**report()['query'], field: changed}}])
            with self.subTest(field=field), self.assertRaisesRegex(RuntimeError, 'admission changed'):
                value.write('old-write')

    def test_reader_only_gets_retained_id_even_after_the_admission_minute(self):
        expected = report()
        value = self.fixture([expected], clock=Mock(return_value=NOW + 180))
        value.read(record(expected))
        self.request.assert_called_once_with('GET', ROUTE + '/' + REPORT_ID)
        value.clock.assert_not_called()

    def test_pending_worker_may_finish_with_recomputed_observation_timestamps(self):
        pending = report()
        ready = report('ready', at=NOW + 180)
        value = self.fixture([ready])
        value.read(record(pending))
        self.request.assert_called_once_with('GET', ROUTE + '/' + REPORT_ID)

    def test_pending_effective_range_may_advance_as_worker_claims_it(self):
        pending = report()
        pending['query']['to'] = NOW
        pending['effective_range']['to'] = NOW - 120
        ready = deepcopy(pending)
        ready.update(status='ready', report_complete=True, summary={},
                     as_of=NOW + 180, measurement_cutoff=NOW + 60, expires_at=NOW + 780)
        ready['effective_range']['to'] = NOW
        self.fixture([ready]).read(record(pending))

    def test_ready_report_requires_complete_endpoint_responses(self):
        base = report('ready', stage='new-write')
        value = self.fixture([base, base, base, *endpoint_shapes()])
        observed = value.write('new-write')
        self.assertEqual(set(observed['ready_shapes']), {'page', 'series', 'owners'})
        self.assertEqual(COLLECTIONS, ['sandbox_usage_reports', 'sandbox_usage_report_control'])

    def test_ready_snapshot_cannot_change_or_become_pending(self):
        expected = report('ready')
        for key, replacement in [('summary', {'changed': 1}), ('warnings', ['changed']),
                                 ('expires_at', NOW + 601), ('report_complete', False),
                                 ('status', 'pending')]:
            with self.subTest(key=key), self.assertRaises(RuntimeError):
                self.fixture([{**expected, key: replacement}]).read(record(expected))

    def test_reader_rejects_changed_immutable_fields(self):
        expected = report()
        for field, replacement in [('schema_version', 2), ('report_id', 'umr_' + 'd' * 32),
                                   ('query', {**expected['query'], 'step_seconds': 120}),
                                   ('scope', {'admin': True, 'principal': 'other'}),
                                   ('poll_url', '/wrong')]:
            with self.subTest(field=field), self.assertRaises(RuntimeError):
                self.fixture([{**expected, field: replacement}]).read(record(expected))

    def test_failed_or_incomplete_reports_are_rejected(self):
        for response in [report('failed'), {**report('ready'), 'report_complete': False},
                         {**report('ready'), 'summary': None},
                         {**report(), 'report_complete': True}]:
            with self.subTest(response=response), self.assertRaises(RuntimeError):
                self.fixture([response]).write('old-write')

    def test_malformed_reports_are_contextual_errors(self):
        for response in [None, [], {}, {**report(), 'schema_version': True},
                         {**report(), 'scope': None}, {**report(), 'query': {}},
                         {**report(), 'as_of': '123'}, {**report(), 'measurement_cutoff': NOW},
                         {**report(), 'effective_range': {}},
                         {**report(), 'report_id': 'umr_../unsafe'}]:
            with self.subTest(response=response), self.assertRaisesRegex(RuntimeError, 'usage migration:'):
                self.fixture([response]).write('old-write')

    def test_reader_checks_ready_endpoint_values_exactly(self):
        expected = record(report('ready'))
        expected['ready_shapes'] = dict(zip(('page', 'series', 'owners'), endpoint_shapes()))
        self.fixture([expected['observed'], *endpoint_shapes()]).read(expected)
        for index, key in enumerate(('items', 'lines', 'items')):
            values = endpoint_shapes()
            values[index][key] = [{'changed': True}]
            with self.subTest(index=index), self.assertRaisesRegex(RuntimeError, 'rows or charts changed'):
                self.fixture([expected['observed'], *values]).read(expected)

    def test_malformed_ready_endpoint_cannot_be_counted_as_shape_evidence(self):
        for index in range(3):
            for invalid in (None, [], {}, {'report_id': 'umr_' + 'e' * 32,
                                          'items': [], 'lines': []}):
                values = endpoint_shapes()
                values[index] = invalid
                with self.subTest(index=index, invalid=invalid), self.assertRaisesRegex(
                        RuntimeError, 'ready report .* shape is invalid'):
                    self.fixture(values).ready_shapes(ROUTE, REPORT_ID)

    def test_both_readers_use_the_correct_writer_record_without_admission(self):
        for writer, reader in [('old-write', 'new-read-old-write'),
                               ('new-write', 'old-read-new-write')]:
            expected = report(stage=writer)
            value = self.fixture([expected] * 3)
            result = value.exercise(writer, [])
            self.assertEqual(result['collections'], COLLECTIONS)
            self.assertTrue(result['checks']['usage_report_same_minute_coalescing'])
            saved = json.loads((self.output / (writer + '-usage.private.json')).read_text())
            self.assertEqual(saved['observed'], expected)
            value = self.fixture([expected])
            result = value.exercise(reader, [])
            self.assertTrue(all(result['checks'].values()))
            self.assertNotIn('usage_report_same_minute_coalescing', result['checks'])
            self.assertEqual(result['shapes'], ['usage-report-admission'])
            self.request.assert_called_once_with('GET', ROUTE + '/' + REPORT_ID)

    def test_stage_route_and_http_errors_fail_without_a_success_receipt(self):
        value = self.fixture([])
        with self.assertRaisesRegex(RuntimeError, 'unsupported stage'):
            value.exercise('unknown', [])
        with self.assertRaisesRegex(RuntimeError, 'unexpected retained report route'):
            value.read({**record(report()), 'route': '/unexpected'})
        self.request.side_effect = RuntimeError('HTTP failure')
        with self.assertRaisesRegex(RuntimeError, 'HTTP failure'):
            value.exercise('old-write', [])
        self.assertFalse(list(self.output.iterdir()))


if __name__ == '__main__':
    unittest.main()
