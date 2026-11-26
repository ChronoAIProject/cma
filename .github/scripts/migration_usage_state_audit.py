"""Audit saved nonempty usage views for admitted, unprovisioned Sandboxes.

This validates one declared sandbox_usage shape. It neither establishes event
history or metering under allocation nor infers persistence from an empty page.
The caller must bind raw traffic and admissions to a frozen image campaign.
"""
from datetime import datetime
import math
import re

from migration_browser_fixture import OWNER
from migration_profile_fixture import usage_snapshot


ROUTES = {'owner': '/api/v1/usage', 'admin': '/api/v1/admin/usage',
          'owner_page': '/api/v1/usage/sandboxes',
          'admin_page': '/api/v1/admin/usage/sandboxes'}
TOTALS = {'uptime_seconds': 0, 'cpu_core_hours': 0.0, 'memory_gib_hours': 0.0,
          'disk_gib_hours': 0.0, 'cpu_millis_seconds': 0, 'memory_mib_seconds': 0,
          'disk_gib_seconds': 0}
ALLOCATION = {'running': False, 'cpu_millis': 0, 'memory_mib': 0, 'disk_gib': 0}


def require(condition, message):
    if not condition:
        raise ValueError('usage state audit: ' + message)


def zero_fields(value, expected, label):
    require(isinstance(value, dict) and set(value) == set(expected), label + ' fields differ')
    for field, exemplar in expected.items():
        require(type(value[field]) is type(exemplar) and value[field] == exemplar,
                label + ' value differs')


def admitted_rows(admissions):
    require(isinstance(admissions, list) and admissions, 'nonempty admissions required')
    result = {}
    for record in admissions:
        require(isinstance(record, dict), 'invalid admitted record')
        stable = record.get('stable')
        require(isinstance(stable, dict), 'admitted identity missing')
        identifier = stable.get('id')
        require(isinstance(identifier, str) and re.fullmatch(r'sbx_[a-f0-9]{32}', identifier)
                and identifier not in result and stable.get('owner') == OWNER
                and isinstance(stable.get('name'), str) and stable['name'],
                'admitted identity differs or repeats')
        clock = stable.get('created_at')
        require(isinstance(clock, str), 'admitted timestamp missing')
        try:
            at = datetime.fromisoformat(clock.replace('Z', '+00:00'))
        except ValueError as error:
            raise ValueError('usage state audit: invalid admission timestamp') from error
        require(at.tzinfo is not None and at.timestamp() >= 0 and at.microsecond == 0,
                'admission timestamp must be whole UTC seconds')
        result[identifier] = {'sandbox_id': identifier, 'owner': OWNER, 'name': stable['name'],
            'created_at': int(at.timestamp()), 'tracking_started_at': int(at.timestamp())}
    return result


def observations(requests):
    require(isinstance(requests, list), 'raw request list missing')
    result, identifiers = {}, set()
    for name, route in ROUTES.items():
        matches = [r for r in requests if isinstance(r, dict) and r.get('route') == route]
        require(len(matches) == 1, 'missing or repeated usage request')
        row = matches[0]
        require(row.get('method') == 'GET' and row.get('status') == 200
                and row.get('body') is None and row.get('idempotency_key') is None,
                'usage read was not successful')
        identifier = row.get('request_id')
        require(isinstance(identifier, str) and identifier and identifier not in identifiers,
                'request identity missing or repeated')
        identifiers.add(identifier)
        start, end = row.get('started_at'), row.get('finished_at')
        require(all(type(n) in (float, int) and math.isfinite(n) and n >= 0 for n in (start, end))
                and end >= start, 'request timing invalid')
        response = row.get('response')
        require(isinstance(response, dict), 'usage response missing')
        result[name] = response
    return result


def validate_page(page, expected):
    require(set(page) == {'items', 'next', 'as_of'} and page['next'] is None
            and isinstance(page['items'], list) and len(page['items']) == len(expected),
            'usage inventory is incomplete or has extra rows')
    seen = set()
    for row in page['items']:
        fields = {'sandbox_id', 'owner', 'name', 'status', 'allocation', 'created_at',
                  'tracking_started_at', 'as_of', 'complete_lifecycle', 'totals'}
        require(isinstance(row, dict) and set(row) == fields, 'usage row fields differ')
        identifier = row.get('sandbox_id')
        require(isinstance(identifier, str) and identifier in expected and identifier not in seen,
                'usage identity differs or repeats')
        seen.add(identifier)
        require(all(type(row[k]) is type(v) and row[k] == v for k, v in expected[identifier].items()),
                'usage admission metadata changed')
        require(row['status'] == 'failed' and row['complete_lifecycle'] is True,
                'unexpected usage lifecycle')
        require(type(row['as_of']) is int and row['as_of'] == page['as_of']
                and row['as_of'] >= row['created_at'], 'usage observation clock invalid')
        zero_fields(row['allocation'], ALLOCATION, 'allocation')
        zero_fields(row['totals'], TOTALS, 'totals')
    require([r['sandbox_id'] for r in page['items']] == sorted(seen), 'usage ordering changed')


def audit_stage(requests, admissions):
    expected = admitted_rows(admissions)
    views = observations(requests)
    for name, value in views.items():
        require(type(value.get('as_of')) is int and value['as_of'] >= 0,
                'invalid usage observation timestamp')
        if name.endswith('_page'):
            validate_page(value, expected)
        else:
            fields = {'as_of', 'measurement', 'sandbox_count', 'running_count',
                      'incomplete_lifecycle_count', 'tracking_started_at', 'totals'}
            require(set(value) == fields and value['measurement'] == 'allocated_resource_time',
                    'usage summary fields differ')
            for key, count in [('sandbox_count', len(expected)), ('running_count', 0),
                               ('incomplete_lifecycle_count', 0),
                               ('tracking_started_at', min(r['created_at'] for r in expected.values()))]:
                require(type(value[key]) is int and value[key] == count, 'usage summary differs')
            require(value['as_of'] >= max(r['created_at'] for r in expected.values()),
                    'usage summary observation precedes admission')
            zero_fields(value['totals'], TOTALS, 'summary totals')
    normalized = {name: usage_snapshot(value, page=name.endswith('_page'))
                  for name, value in views.items()}
    require(normalized['owner'] == normalized['admin']
            and normalized['owner_page'] == normalized['admin_page'], 'owner/admin views differ')
    return normalized


def audit_directions(requests_by_stage, admissions_by_writer):
    stages = ('old-write', 'new-read-old-write', 'new-write', 'old-read-new-write')
    require(isinstance(requests_by_stage, dict) and set(requests_by_stage) == set(stages),
            'all four stages required')
    require(isinstance(admissions_by_writer, dict)
            and set(admissions_by_writer) == {'old-write', 'new-write'}, 'both writers required')
    old, new = admissions_by_writer['old-write'], admissions_by_writer['new-write']
    admitted_rows(new)
    snapshots = {stage: audit_stage(requests_by_stage[stage], old if index < 2 else old + new)
                 for index, stage in enumerate(stages)}
    for writer, reader in (stages[:2], stages[2:]):
        require(snapshots[writer] == snapshots[reader], 'retained usage changed across reader switch')
    return {'collection': 'sandbox_usage', 'shape': 'failed-unprovisioned-zero-allocation',
            'verdict': 'PASS_FOR_DECLARED_PROJECTED_SHAPE',
            'old_writer_records': len(old), 'new_writer_records': len(new),
            'raw_usage_requests': len(stages) * len(ROUTES),
            'stages': {stage: len(snapshots[stage]['owner_page']['items']) for stage in stages},
            'limits': ['Internal nonprojected fields are not compared',
                       'No nonzero allocation, metering or event-history claim',
                       'No boot or complete collection coverage verdict']}
