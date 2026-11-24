#!/usr/bin/env python3
"""Require populated, retained usage-report rows and chart membership markers.

Compose with the existing Sandbox admission fixture and its read-only runtime
RBAC. All durable writes go through the application. This module supplies a
future exact-image fixture; unit tests alone confer no compatibility result.
"""
import json
from pathlib import Path
import time
from urllib.parse import urlencode

from migration_profile_fixture import main
from migration_sandbox_fixture import SandboxFixture, STAGES
from migration_usage_fixture import ROUTE, UsageFixture, require


COLLECTIONS = ['sandbox_usage_reports', 'sandbox_usage_report_control',
               'sandbox_usage_report_rows', 'sandbox_usage_report_charts']
SHAPES = ['populated-unprovisioned-report-rows', 'retained-report-pagination',
          'null-measurement-chart-membership']
ROW_FIELDS = {'sandbox_id', 'name', 'owner', 'status', 'runtime_version',
              'agent_count', 'last_activity_at', 'allocation', 'allocated_period',
              'measured', 'billing', 'shared_sandbox_usage'}
POINT_FIELDS = {'from', 'to', 'mean', 'min', 'max', 'covered_seconds',
                'expected_seconds', 'unknown_mapping_seconds', 'numerator',
                'denominator', 'reason', 'excluded_count'}
CAPACITY_FIELDS = {'cpu_cores', 'memory_bytes', 'disk_bytes'}
PERIOD_FIELDS = {'disk_retained_seconds', 'uptime_seconds', 'cpu_core_hours',
                 'memory_gib_hours', 'disk_gib_hours', 'complete',
                 'projected_tail_seconds', 'unknown_seconds'}
MEASUREMENT_FIELDS = {'state', 'observed_at', 'source', 'covered_seconds',
                      'expected_seconds', 'unknown_mapping_seconds', 'coverage_ratio',
                      'available_from', 'last_observed', 'percent_of_limit',
                      'filesystem_capacity_bytes', 'exceeds_capacity'}
# The public serializer adds dimensions; internal Measurement keys never reach
# these endpoints. Retain exact metric-specific fields across the reader switch.
CPU_FIELDS = {'latest_cores', 'mean_cores', 'max_cores', 'observed_core_hours'}
GAUGE_FIELDS = {'latest_bytes', 'mean_bytes', 'max_bytes', 'observed_gib_hours'}


def canonical(value):
    # Python equality equates True with 1; retained JSON types must also survive.
    return json.dumps(value, sort_keys=True, allow_nan=False, separators=(',', ':'))


class ReportRowsFixture:
    def __init__(self, fixture, clock=time.time, sleep=time.sleep):
        self.fixture, self.output = fixture, Path(fixture.output)
        self.sandboxes = SandboxFixture(fixture)
        self.reports = UsageFixture(fixture, clock=clock, sleep=sleep)
        self.clock, self.sleep = clock, sleep

    def exercise(self, stage, profiles):
        require(stage in STAGES, 'unsupported populated-report stage')
        result = self.sandboxes.exercise(stage, profiles)
        writer = stage if stage in ('old-write', 'new-write') else (
            'old-write' if stage == 'new-read-old-write' else 'new-write')
        admission = json.loads((self.output / (writer + '-sandbox-admission.private.json')).read_text())
        require(admission.get('writer') == writer and len(admission.get('rows', [])) == 2,
                'populated report lacks the exact admission writer')
        path = self.output / (writer + '-report-rows.private.json')
        if stage == writer:
            record = self.write(writer, admission['rows'])
            with path.open('x') as stream:
                stream.write(json.dumps(record, indent=2, allow_nan=False) + '\n')
        else:
            record = json.loads(path.read_text())
            require(record.get('writer') == writer, 'retained report writer differs')
            self.read(record, admission['rows'])
        return {'collections': sorted(set(result['collections'] + COLLECTIONS)),
                'shapes': result['shapes'] + SHAPES, 'samples': result['samples'] + 2,
                'checks': {**result['checks'], **{shape: True for shape in SHAPES}}}

    def write(self, writer, admissions):
        # Inventory selects current retained Sandbox identities independently of
        # the historical measurement range. A past interval avoids a moving
        # cutoff and deliberately exercises unmeasured, zero-allocation rows.
        end = int(self.clock()) // 60 * 60 - 600
        prefix = 'matrix-admission-' + writer + '-'
        require(all(row['stable']['name'].startswith(prefix) for row in admissions),
                'admission names do not match the report filter')
        body = {'from': end - 600, 'to': end, 'step_seconds': 60,
                'filters': {'search': prefix}}
        created = self.fixture.admin_request('POST', ROUTE, body=body, status=(200, 202))
        identifier = created.get('report_id') if isinstance(created, dict) else None
        self.reports.assert_report(created, identifier)
        require(all(created['query'].get(key) == body[key]
                    for key in ('from', 'to', 'step_seconds'))
                and created['query']['filters'].get('search') == prefix,
                'populated report changed the requested range or filter')
        ready = created
        for _ in range(30):
            if ready['status'] == 'ready':
                break
            # Pending polling is bounded and never re-submits an admission.
            self.sleep(2)
            ready = self.fixture.admin_request('GET', ROUTE + '/' + identifier)
            self.reports.assert_preserved(created, ready)
        require(ready['status'] == 'ready', 'populated report did not finish within 30 polls')
        require(ready['summary'].get('sandbox_count') == len(admissions),
                'report summary has the wrong Sandbox count')
        snapshot = self.snapshot(ready, admissions)
        return {'writer': writer, 'body': body, 'created': created,
                'ready': ready, 'snapshot': snapshot}

    def read(self, record, admissions):
        expected = record['ready']
        self.reports.assert_report(expected, expected.get('report_id'))
        require(expected['status'] == 'ready', 'retained populated report was never ready')
        actual = self.fixture.admin_request('GET', ROUTE + '/' + expected['report_id'])
        self.reports.assert_preserved(expected, actual)
        require(canonical(actual) == canonical(expected), 'retained report JSON types changed')
        require(canonical(self.snapshot(actual, admissions)) == canonical(record['snapshot']),
                'retained report rows, pages, owners or charts changed')

    def snapshot(self, report, admissions):
        expected = {row['stable']['id']: row['stable'] for row in admissions}
        require(len(expected) == 2, 'report needs two distinct application-written Sandboxes')
        identifier = report['report_id']
        route = ROUTE + '/' + identifier
        pages, rows, seen_cursors = [], {}, set()
        cursor = None
        for _ in range(len(expected) + 1):
            query = {'limit': 1, 'sort': 'name', 'order': 'asc'}
            if cursor:
                query['cursor'] = cursor
            page = self.fixture.admin_request('GET', route + '/sandboxes?' + urlencode(query))
            require(isinstance(page, dict) and set(page) == {
                'items', 'next', 'report_id', 'as_of', 'total_count'}, 'invalid report page fields')
            require(page['report_id'] == identifier and type(page['as_of']) is int
                    and page['as_of'] == report['as_of']
                    and type(page['total_count']) is int and page['total_count'] == len(expected)
                    and isinstance(page['items'], list) and len(page['items']) == 1,
                    'empty, truncated or incorrectly bound report page')
            row = page['items'][0]
            self.assert_row(row, expected)
            require(row['sandbox_id'] not in rows, 'duplicate report Sandbox row')
            rows[row['sandbox_id']] = row
            pages.append(page)
            cursor = page['next']
            if cursor is None:
                break
            require(isinstance(cursor, str) and 0 < len(cursor) <= 4096
                    and cursor not in seen_cursors, 'invalid or repeated report cursor')
            seen_cursors.add(cursor)
        require(set(rows) == set(expected) and cursor is None, 'report omitted retained Sandbox rows')
        owners = self.fixture.admin_request('GET', route + '/owners')
        require(isinstance(owners, dict) and owners.get('report_id') == identifier
                and owners.get('next') is None and isinstance(owners.get('items'), list)
                and len(owners['items']) == 1 and isinstance(owners['items'][0], dict)
                and owners['items'][0].get('subject') == next(iter(expected.values()))['owner'],
                'report owner membership differs')
        charts = {}
        for metric in ('cpu', 'memory', 'disk'):
            chart = self.fixture.admin_request('GET', route + '/series?' + urlencode(
                {'metric': metric, 'group_by': 'sandbox', 'top': 20}))
            self.assert_chart(chart, report, metric, expected)
            charts[metric] = chart
        snapshot = {'pages': pages, 'owners': owners, 'charts': charts}
        canonical(snapshot)
        return snapshot

    @staticmethod
    def assert_row(row, expected):
        require(isinstance(row, dict) and set(row) == ROW_FIELDS, 'incomplete report row')
        require(row['sandbox_id'] in expected, 'foreign report Sandbox identity')
        original = expected[row['sandbox_id']]
        require(row['name'] == original['name'] and isinstance(row['owner'], dict)
                and row['owner'].get('subject') == original['owner'], 'report row identity changed')
        require(row['status'] in ('provisioning', 'failed') and row['runtime_version'] is None
                and type(row['agent_count']) is int and row['agent_count'] == 0
                and type(row['shared_sandbox_usage']) is bool, 'unexpected report runtime state')
        require(isinstance(row['allocation'], dict) and set(row['allocation']) == CAPACITY_FIELDS
                and isinstance(row['allocated_period'], dict)
                and set(row['allocated_period']) == PERIOD_FIELDS
                and isinstance(row['measured'], dict) and set(row['measured']) == {'cpu', 'memory', 'disk'}
                and isinstance(row['billing'], dict)
                and set(row['billing']) == {'basis', 'status', 'charges'}, 'missing nested report fields')
        for metric, value in row['measured'].items():
            fields = CPU_FIELDS if metric == 'cpu' else GAUGE_FIELDS
            require(isinstance(value, dict) and set(value) == MEASUREMENT_FIELDS | fields,
                    'invalid public measurement fields for ' + metric)

    @staticmethod
    def assert_chart(chart, report, metric, expected):
        require(isinstance(chart, dict) and chart.get('report_id') == report['report_id']
                and chart.get('metric') == metric and isinstance(chart.get('unit'), str)
                and isinstance(chart.get('source'), str) and isinstance(chart.get('lines'), list)
                and len(chart['lines']) == len(expected), 'empty or incorrectly bound report chart')
        seen = set()
        for line in chart['lines']:
            require(isinstance(line, dict) and line.get('id') in expected
                    and line['id'] not in seen and isinstance(line.get('label'), str)
                    and isinstance(line.get('points'), list) and line['points'],
                    'chart membership missing or duplicated')
            seen.add(line['id'])
            until = report['effective_range']['from']
            for point in line['points']:
                require(isinstance(point, dict) and set(point) == POINT_FIELDS,
                        'incomplete chart point')
                require(type(point['from']) is int and type(point['to']) is int
                        and point['from'] == until and point['from'] < point['to']
                        <= min(report['effective_range']['to'], until + report['query']['step_seconds']),
                        'chart range has a gap or overlap')
                require(all(point[field] is None for field in ('mean', 'min', 'max', 'numerator', 'denominator')),
                        'unexpected non-null measurement in unprovisioned fixture')
                until = point['to']
            require(until == report['effective_range']['to'], 'chart range is truncated')


if __name__ == '__main__':
    main(ReportRowsFixture)
