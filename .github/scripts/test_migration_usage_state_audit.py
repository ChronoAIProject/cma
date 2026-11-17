"""Faults in recorded usage traffic must not become migration passes."""
from copy import deepcopy
import unittest

from migration_browser_fixture import OWNER
from migration_usage_state_audit import ALLOCATION, ROUTES, TOTALS, audit_directions, audit_stage


def admission(character, name):
    return {'stable': {'id': 'sbx_' + character * 32, 'owner': OWNER, 'name': name,
                       'created_at': '2026-01-01T00:00:00Z'}}


def traffic(admissions, clock=1767225700):
    rows = [{'sandbox_id': a['stable']['id'], 'owner': OWNER, 'name': a['stable']['name'],
        'status': 'failed', 'allocation': deepcopy(ALLOCATION), 'created_at': 1767225600,
        'tracking_started_at': 1767225600, 'as_of': clock, 'complete_lifecycle': True,
        'totals': deepcopy(TOTALS)} for a in sorted(admissions, key=lambda a: a['stable']['id'])]
    summary = {'as_of': clock, 'measurement': 'allocated_resource_time',
        'sandbox_count': len(rows), 'running_count': 0, 'incomplete_lifecycle_count': 0,
        'tracking_started_at': 1767225600, 'totals': deepcopy(TOTALS)}
    return [{'route': route, 'method': 'GET', 'status': 200, 'body': None,
        'idempotency_key': None, 'request_id': 'request-' + name, 'started_at': float(clock),
        'finished_at': clock + 0.1, 'response': deepcopy(
            {'items': rows, 'next': None, 'as_of': clock} if name.endswith('_page') else summary)}
        for name, route in ROUTES.items()]


class UsageStateAuditTests(unittest.TestCase):
    def setUp(self):
        self.admissions = [admission('a', 'old'), admission('b', 'second')]
        self.requests = traffic(self.admissions)

    def test_both_directions_preserve_nonempty_pages_without_counting_clocks_as_data(self):
        old, new = self.admissions, [admission('c', 'new'), admission('d', 'second-new')]
        raw = {'old-write': traffic(old), 'new-read-old-write': traffic(old, 1767225800),
               'new-write': traffic(old + new), 'old-read-new-write': traffic(old + new, 1767225900)}
        result = audit_directions(raw, {'old-write': old, 'new-write': new})
        self.assertEqual(result['raw_usage_requests'], 16)
        self.assertEqual(result['collection'], 'sandbox_usage')
        self.assertEqual(list(result['stages'].values()), [2, 2, 4, 4])
        self.assertEqual(result['old_writer_records'], 2)
        self.assertEqual(result['new_writer_records'], 2)

    def test_empty_missing_duplicate_or_extra_admissions_are_not_persistence_proof(self):
        for rows in ([], self.admissions[:1], self.admissions * 2,
                     self.admissions + [admission('c', 'extra')]):
            with self.subTest(rows=rows), self.assertRaises(ValueError):
                audit_stage(self.requests, rows)

    def test_every_admission_identity_and_timestamp_is_bound(self):
        cases = [('id', 'bad'), ('owner', 'other'), ('name', ''), ('created_at', None),
                 ('created_at', 'bad'), ('created_at', '2026-01-01'),
                 ('created_at', '2026-01-01T00:00:00.1Z')]
        for key, value in cases:
            admissions = deepcopy(self.admissions)
            admissions[0]['stable'][key] = value
            with self.subTest(field=key, value=value), self.assertRaises(ValueError):
                audit_stage(self.requests, admissions)
        for invalid in (None, [None], [{}]):
            with self.assertRaises(ValueError):
                audit_stage(self.requests, invalid)

    def test_raw_request_identity_success_and_timing_are_required(self):
        cases = [('method', 'POST'), ('status', 503), ('body', {}), ('idempotency_key', 'write'),
                 ('request_id', ''), ('request_id', self.requests[1]['request_id']),
                 ('started_at', True), ('started_at', float('nan')), ('finished_at', 0),
                 ('response', None)]
        for key, value in cases:
            raw = deepcopy(self.requests); raw[0][key] = value
            with self.subTest(field=key, value=value), self.assertRaises(ValueError):
                audit_stage(raw, self.admissions)
        for raw in (None, self.requests[:3], self.requests * 2):
            with self.assertRaises(ValueError):
                audit_stage(raw, self.admissions)

    def test_every_projected_row_field_is_checked_and_unknown_fields_fail(self):
        original = self.requests[2]['response']['items'][0]
        for key in (*original, 'unexpected'):
            raw = deepcopy(self.requests)
            raw[2]['response']['items'][0][key] = 'changed'
            with self.subTest(field=key), self.assertRaises(ValueError):
                audit_stage(raw, self.admissions)

    def test_allocation_and_totals_types_cannot_hide_boolean_or_nonzero_mutations(self):
        for nested in ('allocation', 'totals'):
            for key in self.requests[2]['response']['items'][0][nested]:
                raw = deepcopy(self.requests)
                raw[2]['response']['items'][0][nested][key] = True
                with self.subTest(nested=nested, field=key), self.assertRaises(ValueError):
                    audit_stage(raw, self.admissions)
        raw = deepcopy(self.requests); raw[2]['response']['items'][0]['totals']['cpu_core_hours'] = 1.0
        with self.assertRaises(ValueError):
            audit_stage(raw, self.admissions)

    def test_page_bound_identity_order_and_observation_are_checked(self):
        changes = [lambda p: p.update(next='more'), lambda p: p['items'].reverse(),
            lambda p: p['items'].pop(), lambda p: p['items'].append(p['items'][0]),
            lambda p: p['items'].__setitem__(1, p['items'][0]),
            lambda p: p['items'][0].update(as_of=True), lambda p: p.update(as_of=-1)]
        for mutate in changes:
            raw = deepcopy(self.requests); mutate(raw[2]['response'])
            with self.subTest(mutation=mutate), self.assertRaises(ValueError):
                audit_stage(raw, self.admissions)

    def test_summary_fields_values_and_types_are_preserved(self):
        for key in (*self.requests[0]['response'], 'unexpected'):
            raw = deepcopy(self.requests); raw[0]['response'][key] = 'changed'
            with self.subTest(field=key), self.assertRaises(ValueError):
                audit_stage(raw, self.admissions)
        raw = deepcopy(self.requests); raw[0]['response']['running_count'] = False
        with self.assertRaises(ValueError):
            audit_stage(raw, self.admissions)
        raw = deepcopy(self.requests); raw[0]['response']['totals']['cpu_core_hours'] = False
        with self.assertRaises(ValueError):
            audit_stage(raw, self.admissions)

    def test_missing_stage_or_reused_writer_identity_is_rejected(self):
        for requests, admissions in (({}, {}), ({stage: self.requests for stage in (
                'old-write', 'new-read-old-write', 'new-write', 'old-read-new-write')},
                {'old-write': self.admissions, 'new-write': self.admissions})):
            with self.assertRaises(ValueError):
                audit_directions(requests, admissions)


if __name__ == '__main__':
    unittest.main()
