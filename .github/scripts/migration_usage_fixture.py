#!/usr/bin/env python3
"""Exercise durable usage-report admission without a native runtime.

The fixture claims only report admission/control documents. Report rows and
charts are claimed when a real ready report supplies those shapes; an empty or
pending report never becomes evidence for those collections by inference.
"""
import json


COLLECTIONS = ['sandbox_usage_reports', 'sandbox_usage_report_control']
SHAPES = ['usage-report-admission', 'usage-report-query-coalescing']


def require(condition, message):
    if not condition:
        raise RuntimeError('usage migration: ' + message)


class UsageFixture:
    def __init__(self, fixture):
        self.fixture = fixture

    def exercise(self, stage, profiles):
        require(stage in ('old-write', 'new-write', 'new-read-old-write',
                          'old-read-new-write'), 'unsupported stage')
        if stage in ('old-write', 'new-write'):
            record = self.write(stage)
            (self.fixture.output / (stage + '-usage.private.json')).write_text(
                json.dumps(record, indent=2) + '\n')
        else:
            writer = 'old-write' if stage == 'new-read-old-write' else 'new-write'
            record = json.loads((self.fixture.output / (writer + '-usage.private.json')).read_text())
            self.read(record)
        return {'collections': COLLECTIONS, 'shapes': SHAPES, 'samples': 1,
                'checks': {'usage_report_admission_preserved': True,
                           'usage_report_query_coalescing_preserved': True}}

    def write(self, stage):
        body = {}
        route = '/api/v1/admin/usage/metrics/reports'
        created = self.fixture.admin_request('POST', route, body=body, status=(200, 202))
        report_id = created.get('report_id')
        require(isinstance(report_id, str) and report_id.startswith('umr_'),
                'report admission returned no immutable report ID')
        observed = self.fixture.admin_request('GET', route + '/' + report_id)
        self.assert_report(observed, report_id)
        replay = self.fixture.admin_request('POST', route, body=body, status=(200, 202))
        require(replay.get('report_id') == report_id,
                'same canonical query did not coalesce to the retained report')
        result = {'route': route, 'body': body, 'report_id': report_id,
                  'created': created, 'observed': observed, 'replay': replay}
        if observed.get('status') == 'ready':
            result['ready_shapes'] = self.ready_shapes(route, report_id)
        return result

    def read(self, record):
        route, report_id = record['route'], record['report_id']
        observed = self.fixture.admin_request('GET', route + '/' + report_id)
        self.assert_report(observed, report_id)
        expected = record['observed']
        # A pending job may legitimately finish during a reader switch. All
        # immutable query/scope fields must survive; terminal failure is not a
        # compatible read.
        for field in ('schema_version', 'report_id', 'scope', 'query', 'effective_range',
                      'poll_url'):
            require(observed.get(field) == expected.get(field),
                    'report field changed across reader switch: ' + field)
        require(observed.get('status') in ('pending', 'ready'),
                'report became failed across reader switch')
        replay = self.fixture.admin_request('POST', route, body=record['body'],
                                            status=(200, 202))
        require(replay.get('report_id') == report_id,
                'same canonical query selected a different report after reader switch')
        if 'ready_shapes' in record:
            require(self.ready_shapes(route, report_id) == record['ready_shapes'],
                    'ready report rows or charts changed across reader switch')

    def assert_report(self, report, report_id):
        require(report.get('report_id') == report_id, 'report ID changed')
        require(report.get('schema_version') == 1, 'unexpected report schema version')
        require(report.get('scope', {}).get('admin') is True,
                'admin report scope was not retained')
        require(isinstance(report.get('query'), dict)
                and report['query'].get('from') < report['query'].get('to'),
                'report query range is invalid')
        require(report.get('status') in ('pending', 'ready'),
                'report admission returned a terminal failure')

    def ready_shapes(self, route, report_id):
        page = self.fixture.admin_request('GET', route + '/' + report_id + '/sandboxes?limit=50')
        series = self.fixture.admin_request('GET', route + '/' + report_id + '/series?metric=cpu')
        owners = self.fixture.admin_request('GET', route + '/' + report_id + '/owners')
        require(isinstance(page.get('items'), list) and page.get('report_id') == report_id,
                'ready report page shape is invalid')
        require(isinstance(series.get('lines'), list) and series.get('report_id') == report_id,
                'ready report series shape is invalid')
        require(isinstance(owners.get('items'), list) and owners.get('report_id') == report_id,
                'ready report owners shape is invalid')
        return {'page': page, 'series': series, 'owners': owners}


def main():
    raise RuntimeError('usage fixture must be composed with migration_composite_fixture.py')


if __name__ == '__main__':
    main()
