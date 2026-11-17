"""Reject empty or corrupted report evidence before certifying a reader switch."""
from copy import deepcopy
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch
from urllib.parse import parse_qs, urlsplit

from migration_report_rows_fixture import COLLECTIONS, ReportRowsFixture
from migration_sandbox_fixture import STAGES
from migration_usage_fixture import ROUTE
from test_migration_sandbox_fixture import PROFILES, SandboxAPI


NOW = 1_200_000


class ReportsAPI(SandboxAPI):
    def __init__(self, output):
        super().__init__(output)
        self.reports, self.calls = {}, []
        self.pending_polls = 0

    def admin_request(self, method, route, body=None, status=200):
        self.calls.append((method, route))
        parts = urlsplit(route)
        if method == 'POST':
            identifier = 'umr_' + format(len(self.reports) + 1, '032x')
            selected = [s for s in self.rows.values() if s['name'].startswith(body['filters']['search'])]
            rows = [self.report_row(s) for s in selected]
            report = {'schema_version': 1, 'report_id': identifier, 'status': 'ready',
                      'report_complete': True, 'scope': {'admin': True, 'principal': 'admin'},
                      'query': body, 'effective_range': {k: body[k] for k in ('from', 'to')},
                      'as_of': NOW, 'measurement_cutoff': NOW - 120, 'expires_at': NOW + 600,
                      'summary': {'sandbox_count': len(rows)}, 'poll_url': ROUTE + '/' + identifier}
            self.reports[identifier] = {'report': report, 'rows': rows}
        identifier = parts.path.split('/')[7] if method == 'GET' else identifier
        saved = self.reports[identifier]
        if parts.path.endswith('/sandboxes'):
            query = parse_qs(parts.query)
            offset = int(query.get('cursor', ['0'])[0])
            rows = sorted(saved['rows'], key=lambda r: r['name'])
            return deepcopy({'report_id': identifier, 'as_of': NOW, 'total_count': len(rows),
                             'items': rows[offset:offset + 1],
                             'next': str(offset + 1) if offset + 1 < len(rows) else None})
        if parts.path.endswith('/owners'):
            return {'report_id': identifier, 'next': None,
                    'items': [deepcopy(saved['rows'][0]['owner'])]}
        if parts.path.endswith('/series'):
            metric = parse_qs(parts.query)['metric'][0]
            report = saved['report']
            points = [{'from': at, 'to': at + 60, 'mean': None, 'min': None, 'max': None,
                       'covered_seconds': 0.0, 'expected_seconds': 0.0,
                       'unknown_mapping_seconds': 0.0, 'numerator': None, 'denominator': None,
                       'reason': 'not_applicable', 'excluded_count': 0}
                      for at in range(report['query']['from'], report['query']['to'], 60)]
            return {'report_id': identifier, 'metric': metric, 'unit': 'test-unit',
                    'source': 'test-fixture', 'lines': [{'id': r['sandbox_id'], 'label': r['name'],
                        'points': deepcopy(points)} for r in saved['rows']]}
        report = deepcopy(saved['report'])
        if self.pending_polls:
            self.pending_polls -= 1
            report.update(status='pending', report_complete=False, summary=None)
        return report

    @staticmethod
    def report_row(sandbox):
        measurement = {'state': 'not_running', 'observed_at': None, 'source': 'test-fixture',
                       'covered_seconds': 0.0, 'expected_seconds': 0.0, 'unknown_mapping_seconds': 0.0,
                       'coverage_ratio': None, 'available_from': None, 'last_observed': None,
                       'percent_of_limit': None, 'filesystem_capacity_bytes': None,
                       'exceeds_capacity': False}
        # Match layer1-public sandbox_metrics/wire.rs, including its units.
        # Internal Measurement names are deliberately absent from this API.
        cpu = {**measurement, 'latest_cores': None, 'mean_cores': None,
               'max_cores': None, 'observed_core_hours': None}
        gauge = {**measurement, 'latest_bytes': None, 'mean_bytes': None,
                 'max_bytes': None, 'observed_gib_hours': None}
        return {'sandbox_id': sandbox['id'], 'name': sandbox['name'],
                'owner': {'subject': sandbox['owner']}, 'status': 'failed',
                'runtime_version': None, 'agent_count': 0, 'last_activity_at': None,
                'allocation': {'cpu_cores': 0.0, 'memory_bytes': 0.0, 'disk_bytes': 0.0},
                'allocated_period': {'complete': True, 'disk_retained_seconds': 0.0,
                    'uptime_seconds': 0.0, 'cpu_core_hours': 0.0, 'memory_gib_hours': 0.0,
                    'disk_gib_hours': 0.0, 'projected_tail_seconds': 0, 'unknown_seconds': 0},
                'measured': {'cpu': cpu, 'memory': deepcopy(gauge), 'disk': deepcopy(gauge)},
                'billing': {'basis': 'allocated', 'status': 'not_integrated', 'charges': None},
                'shared_sandbox_usage': False}


class ReportRowsTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.output = Path(directory.name)
        self.api = ReportsAPI(self.output)
        self.pause = Mock()
        self.fixture = ReportRowsFixture(self.api, clock=lambda: NOW, sleep=self.pause)
        self.profiles = deepcopy(PROFILES)

    def write(self):
        return self.fixture.exercise('old-write', self.profiles)

    def mutate_response(self, suffix, mutate):
        original = self.api.admin_request
        def request(method, route, **kwargs):
            value = original(method, route, **kwargs)
            if suffix in route:
                mutate(value)
            return value
        return patch.object(self.api, 'admin_request', side_effect=request)

    def test_four_stages_read_both_writer_snapshots_without_resubmitting(self):
        for stage in STAGES:
            before = len([c for c in self.api.calls if c[0] == 'POST'])
            result = self.fixture.exercise(stage, self.profiles)
            self.assertTrue(set(COLLECTIONS) <= set(result['collections']))
            self.assertTrue(all(result['checks'].values()))
            self.assertEqual(len([c for c in self.api.calls if c[0] == 'POST']) - before,
                             int(stage in ('old-write', 'new-write')))
        self.assertEqual(len(self.api.reports), 2)
        self.assertEqual(len(self.api.rows), 4)
        for path in self.output.glob('*-report-rows.private.json'):
            saved = json.loads(path.read_text())
            self.assertEqual(len(saved['snapshot']['pages']), 2)
            self.assertEqual(set(saved['snapshot']['charts']), {'cpu', 'memory', 'disk'})

    def test_pending_report_is_polled_every_two_seconds_without_write_retry(self):
        self.api.pending_polls = 3
        self.write()
        self.assertEqual(self.pause.call_args_list, [((2,),)] * 3)
        self.assertEqual(sum(method == 'POST' for method, _ in self.api.calls), 1)

    def test_pending_deadline_cannot_leave_success_receipt(self):
        self.api.pending_polls = 99
        with self.assertRaisesRegex(RuntimeError, 'within 30 polls'):
            self.write()
        self.assertEqual(self.pause.call_count, 30)
        self.assertFalse(list(self.output.glob('*-report-rows.private.json')))

    def test_terminal_report_failure_is_not_retried_or_saved_as_ready(self):
        self.api.pending_polls = 2
        with self.mutate_response(ROUTE, lambda r: r.update(status='failed', report_complete=False)):
            with self.assertRaisesRegex(RuntimeError, 'terminal failure'):
                self.write()
        self.assertEqual(len(self.api.calls), 1)
        self.assertFalse(list(self.output.glob('*-report-rows.private.json')))

    def test_report_admission_errors_are_fatal_and_never_retried(self):
        with patch.object(self.api, 'admin_request', side_effect=RuntimeError('HTTP 503')) as call:
            with self.assertRaisesRegex(RuntimeError, 'HTTP 503'):
                self.write()
        self.assertEqual(call.call_count, 1)

    def test_empty_truncated_duplicate_or_foreign_pages_are_rejected(self):
        self.write()
        mutations = [lambda p: p.update(items=[]), lambda p: p.update(next=None),
                     lambda p: p.update(total_count=1), lambda p: p.update(as_of=NOW + 1),
                     lambda p: p.update(report_id='wrong'),
                     lambda p: p['items'][0].update(sandbox_id='foreign'),
                     lambda p: p['items'][0].pop('measured'),
                     lambda p: p['items'][0].update(shared_sandbox_usage=0)]
        for mutate in mutations:
            with self.subTest(mutate=mutate), self.mutate_response('/sandboxes?', mutate):
                with self.assertRaises(RuntimeError):
                    self.fixture.exercise('new-read-old-write', [])

    def test_cursor_loop_and_duplicate_row_cannot_complete_snapshot(self):
        self.write()
        first = deepcopy(next(iter(self.api.reports.values()))['rows'][0])
        for mutate in (lambda p: p.update(next='1'), lambda p: p.update(items=[first])):
            with self.subTest(mutate=mutate), self.mutate_response('/sandboxes?', mutate):
                with self.assertRaisesRegex(RuntimeError, 'repeated report cursor|duplicate report'):
                    self.fixture.exercise('new-read-old-write', [])

    def test_owner_and_all_nested_values_must_survive_the_reader_switch(self):
        self.write()
        mutations = [lambda p: p['items'][0]['owner'].update(subject='foreign'),
                     lambda p: p['items'][0]['measured']['cpu'].update(mean_cores=1.0),
                     lambda p: p['items'][0]['allocation'].update(cpu_cores=1.0),
                     lambda p: p['items'][0]['measured']['disk'].pop('source'),
                     lambda p: p['items'][0].update(agent_count=True),
                     lambda p: p['items'][0].update(status='active')]
        for mutate in mutations:
            with self.subTest(mutate=mutate), self.mutate_response('/sandboxes?', mutate):
                with self.assertRaises(RuntimeError):
                    self.fixture.exercise('new-read-old-write', [])
        with self.mutate_response('/owners', lambda p: p.update(items=[])):
            with self.assertRaisesRegex(RuntimeError, 'owner membership'):
                self.fixture.exercise('new-read-old-write', [])

    def test_public_measurement_units_are_required_for_each_metric(self):
        sandbox = {'id': 'sbx_public', 'name': 'public', 'owner': 'owner'}
        row = self.api.report_row(sandbox)
        expected = {sandbox['id']: sandbox}
        # Reproduces the exact field-name mismatch seen on the 12d image.
        self.fixture.assert_row(row, expected)
        for metric, field in [('cpu', 'latest_cores'), ('cpu', 'observed_core_hours'),
                              ('memory', 'mean_bytes'), ('disk', 'observed_gib_hours')]:
            with self.subTest(metric=metric, field=field):
                broken = deepcopy(row)
                broken['measured'][metric].pop(field)
                with self.assertRaisesRegex(RuntimeError, 'measurement fields'):
                    self.fixture.assert_row(broken, expected)
        for metric in ('cpu', 'memory', 'disk'):
            with self.subTest(metric=metric):
                internal = deepcopy(row)
                internal['measured'][metric]['latest'] = None
                with self.assertRaisesRegex(RuntimeError, 'measurement fields'):
                    self.fixture.assert_row(internal, expected)
        swapped = deepcopy(row)
        swapped['measured']['cpu'] = deepcopy(swapped['measured']['memory'])
        with self.assertRaisesRegex(RuntimeError, 'measurement fields'):
            self.fixture.assert_row(swapped, expected)

    def test_writer_rejects_changed_query_and_empty_summary(self):
        for mutate in (lambda r: r['query'].update(step_seconds=120),
                       lambda r: r['summary'].update(sandbox_count=0)):
            # Start with a fresh adapter per assertion: an admission is never
            # retried as a route to success in an actual campaign.
            api = ReportsAPI(self.output)
            fixture = ReportRowsFixture(api, clock=lambda: NOW, sleep=self.pause)
            original = api.admin_request
            def changed(method, route, **kwargs):
                value = original(method, route, **kwargs)
                if method == 'POST':
                    mutate(value)
                return value
            with patch.object(api, 'admin_request', side_effect=changed):
                with self.assertRaises(RuntimeError):
                    fixture.exercise('old-write', deepcopy(PROFILES))

    def test_chart_requires_every_retained_member_and_complete_null_points(self):
        self.write()
        mutations = [lambda c: c.update(lines=[]), lambda c: c.update(metric='other'),
                     lambda c: c['lines'][0].update(points=[]),
                     lambda c: c['lines'][0]['points'].pop(),
                     lambda c: c['lines'][0]['points'][0].update(mean=0.0),
                     lambda c: c['lines'][0]['points'][0].pop('reason'),
                     lambda c: c['lines'][0]['points'][0].update(to=NOW),
                     lambda c: c['lines'][1].update(id=c['lines'][0]['id']),
                     lambda c: c['lines'][0]['points'][0].update(expected_seconds=float('nan'))]
        for mutate in mutations:
            with self.subTest(mutate=mutate), self.mutate_response('/series?', mutate):
                with self.assertRaises((RuntimeError, ValueError)):
                    self.fixture.exercise('new-read-old-write', [])

    def test_frozen_report_body_and_json_types_cannot_change(self):
        self.write()
        identifier = next(iter(self.api.reports))
        self.api.reports[identifier]['report']['summary']['sandbox_count'] = 2.0
        with self.assertRaisesRegex(RuntimeError, 'JSON types changed'):
            self.fixture.exercise('new-read-old-write', [])

    def test_saved_writer_provenance_and_distinct_admissions_are_required(self):
        self.write()
        path = self.output / 'old-write-report-rows.private.json'
        saved = json.loads(path.read_text())
        saved['writer'] = 'new-write'
        path.write_text(json.dumps(saved))
        with self.assertRaisesRegex(RuntimeError, 'writer differs'):
            self.fixture.exercise('new-read-old-write', [])
        with self.assertRaisesRegex(RuntimeError, 'unsupported populated-report'):
            self.fixture.exercise('unknown', [])


if __name__ == '__main__':
    unittest.main()
