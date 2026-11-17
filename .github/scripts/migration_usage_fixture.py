#!/usr/bin/env python3
"""Exercise durable usage-report admission without a native runtime.

The fixture claims only report admission/control documents. Ready endpoint
responses are compared when available, but an empty response does not establish
that row or chart documents were persisted.
"""
import json
import re
import time
import uuid


COLLECTIONS = ['sandbox_usage_reports', 'sandbox_usage_report_control']
SHAPES = ['usage-report-admission']
ROUTE = '/api/v1/admin/usage/metrics/reports'


def require(condition, message):
    if not condition:
        raise RuntimeError('usage migration: ' + message)


class UsageFixture:
    def __init__(self, fixture, clock=time.time, sleep=time.sleep):
        self.fixture = fixture
        self.clock = clock
        self.sleep = sleep

    def exercise(self, stage, profiles):
        require(stage in ('old-write', 'new-write', 'new-read-old-write',
                          'old-read-new-write'), 'unsupported stage')
        checks = {'usage_report_admission_preserved': True}
        shapes = list(SHAPES)
        if stage in ('old-write', 'new-write'):
            record = self.write(stage)
            (self.fixture.output / (stage + '-usage.private.json')).write_text(
                json.dumps(record, indent=2) + '\n')
            checks['usage_report_same_minute_coalescing'] = True
            shapes.append('usage-report-same-minute-query-coalescing')
        else:
            writer = 'old-write' if stage == 'new-read-old-write' else 'new-write'
            record = json.loads((self.fixture.output / (writer + '-usage.private.json')).read_text())
            self.read(record)
            checks['usage_report_retained_id_read'] = True
        return {'collections': COLLECTIONS, 'shapes': shapes, 'samples': 1, 'checks': checks}

    def write(self, stage):
        # Both pinned versions include the admission minute in query_hash.
        # Begin early in a minute and retain the exact query. The time-bound
        # precondition below prevents a boundary crossing becoming a product
        # compatibility failure; it never retries or claims coalescing then.
        now = self.clock()
        if now % 60 > 10:
            self.sleep(61 - now % 60)
        started = self.clock()
        end = int(started) // 60 * 60 - 600
        body = {'from': end - 3600, 'to': end, 'step_seconds': 60,
                'filters': {'search': 'matrix-' + stage + '-' + uuid.uuid4().hex}}
        route = ROUTE
        created = self.fixture.admin_request('POST', route, body=body, status=(200, 202))
        report_id = created.get('report_id') if isinstance(created, dict) else None
        self.assert_report(created, report_id)
        for field in ('from', 'to', 'step_seconds'):
            require(created['query'].get(field) == body[field],
                    'admission changed the explicit query: ' + field)
        require(created['query'].get('filters', {}).get('search') == body['filters']['search'],
                'admission changed the unique writer query')
        replay = self.fixture.admin_request('POST', route, body=body, status=(200, 202))
        finished = self.clock()
        require(int(started) // 60 == int(finished) // 60,
                'coalescing minute precondition missed; no coalescing evidence')
        self.assert_preserved(created, replay)
        observed = self.fixture.admin_request('GET', route + '/' + report_id)
        self.assert_preserved(created, observed)
        result = {'route': route, 'body': body, 'report_id': report_id,
                  'created': created, 'observed': observed, 'replay': replay,
                  'coalescing_window': {'started_at': started, 'finished_at': finished}}
        if observed.get('status') == 'ready':
            result['ready_shapes'] = self.ready_shapes(route, report_id)
        return result

    def read(self, record):
        route, report_id = record['route'], record['report_id']
        require(route == ROUTE, 'unexpected retained report route')
        self.assert_report(record['observed'], report_id)
        observed = self.fixture.admin_request('GET', route + '/' + report_id)
        expected = record['observed']
        # Read the saved ID after switching images. Re-submitting after a new
        # minute is a new admission, not a read of the retained document.
        self.assert_preserved(expected, observed)
        if 'ready_shapes' in record:
            require(self.ready_shapes(route, report_id) == record['ready_shapes'],
                    'ready report rows or charts changed across reader switch')

    def assert_preserved(self, expected, observed):
        self.assert_report(observed, expected['report_id'])
        for field in ('schema_version', 'report_id', 'scope', 'query', 'poll_url'):
            require(observed[field] == expected[field],
                    'report field changed across reader switch: ' + field)
        # A pending job can be claimed and finalized across a reader switch;
        # claim_report updates as_of, cutoff, effective_range and expires_at.
        # Ready snapshots are immutable and must match every returned field.
        if expected['status'] == 'ready':
            require(observed == expected, 'ready report snapshot changed')

    def assert_report(self, report, report_id):
        require(isinstance(report_id, str) and
                re.fullmatch(r'umr_[a-f0-9]{32}', report_id), 'invalid report ID')
        require(isinstance(report, dict), 'report is not an object')
        require(report.get('report_id') == report_id, 'report ID changed')
        require(type(report.get('schema_version')) is int and report['schema_version'] == 1,
                'unexpected report schema version')
        scope = report.get('scope')
        require(isinstance(scope, dict) and scope.get('admin') is True
                and isinstance(scope.get('principal'), str) and scope['principal'],
                'admin report scope was not retained')
        query = report.get('query')
        require(isinstance(query, dict)
                and all(type(query.get(key)) is int for key in ('from', 'to', 'step_seconds'))
                and 0 <= query['from'] < query['to'] and query['step_seconds'] > 0
                and isinstance(query.get('filters'), dict),
                'report query range is invalid')
        require(report.get('status') in ('pending', 'ready'),
                'report admission returned a terminal failure')
        require(report.get('report_complete') is (report['status'] == 'ready'),
                'report completion flag disagrees with status')
        require(report.get('poll_url') == ROUTE + '/' + report_id,
                'report poll URL changed')
        require(all(type(report.get(key)) is int and report[key] >= 0
                    for key in ('as_of', 'measurement_cutoff', 'expires_at')),
                'report timestamps are invalid')
        require(report['measurement_cutoff'] == max(0, report['as_of'] - 120)
                and report['expires_at'] > report['as_of'], 'report time bounds are invalid')
        require(report.get('effective_range') == {
            'from': query['from'],
            'to': max(query['from'], min(query['to'], report['measurement_cutoff']))},
            'report effective range is invalid')
        if report['status'] == 'ready':
            require(isinstance(report.get('summary'), dict), 'ready report summary missing')

    def ready_shapes(self, route, report_id):
        page = self.fixture.admin_request('GET', route + '/' + report_id + '/sandboxes?limit=50')
        series = self.fixture.admin_request('GET', route + '/' + report_id + '/series?metric=cpu')
        owners = self.fixture.admin_request('GET', route + '/' + report_id + '/owners')
        require(isinstance(page, dict) and isinstance(page.get('items'), list)
                and page.get('report_id') == report_id,
                'ready report page shape is invalid')
        require(isinstance(series, dict) and isinstance(series.get('lines'), list)
                and series.get('report_id') == report_id,
                'ready report series shape is invalid')
        require(isinstance(owners, dict) and isinstance(owners.get('items'), list)
                and owners.get('report_id') == report_id,
                'ready report owners shape is invalid')
        return {'page': page, 'series': series, 'owners': owners}


def main():
    raise RuntimeError('usage fixture must be composed with migration_composite_fixture.py')


if __name__ == '__main__':
    main()
