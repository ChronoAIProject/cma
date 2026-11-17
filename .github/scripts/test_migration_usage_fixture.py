#!/usr/bin/env python3
"""Guard usage-report evidence against transient or incomplete shapes."""
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock

from migration_usage_fixture import UsageFixture


def report(status='pending'):
    return {'schema_version': 1, 'report_id': 'umr_' + 'a' * 32, 'status': status,
            'scope': {'principal': 'owner', 'admin': True},
            'query': {'from': 100, 'to': 200}, 'effective_range': {'from': 100, 'to': 200},
            'poll_url': '/api/v1/admin/usage/metrics/reports/umr_' + 'a' * 32}


class UsageFixtureTests(unittest.TestCase):
    def fixture(self, response):
        request = Mock(side_effect=[response, response, response])
        return UsageFixture(SimpleNamespace(output=Path('/tmp'), admin_request=request)), request

    def test_pending_report_preserves_identity_and_coalesces(self):
        value, request = self.fixture(report())
        record = value.write('old-write')
        self.assertEqual(record['report_id'], report()['report_id'])
        self.assertNotIn('ready_shapes', record)
        self.assertEqual(request.call_count, 3)

    def test_ready_report_requires_complete_page_series_and_owner_shapes(self):
        base = report('ready')
        page = {'items': [], 'report_id': base['report_id']}
        series = {'lines': [], 'report_id': base['report_id']}
        owners = {'items': [], 'report_id': base['report_id']}
        request = Mock(side_effect=[base, base, base, page, series, owners])
        value = UsageFixture(SimpleNamespace(output=Path('/tmp'), admin_request=request))
        record = value.write('new-write')
        self.assertEqual(set(record['ready_shapes']), {'page', 'series', 'owners'})

    def test_failed_report_is_not_compatibility_evidence(self):
        value, _ = self.fixture(report('failed'))
        with self.assertRaisesRegex(RuntimeError, 'terminal failure'):
            value.write('old-write')

    def test_reader_rejects_changed_query_or_report_identity(self):
        expected = report()
        request = Mock(side_effect=[{**expected, 'query': {'from': 101, 'to': 200}}])
        value = UsageFixture(SimpleNamespace(output=Path('/tmp'), admin_request=request))
        with self.assertRaisesRegex(RuntimeError, 'report field changed'):
            value.read({'route': '/api/v1/admin/usage/metrics/reports',
                        'report_id': expected['report_id'], 'body': {}, 'observed': expected})


if __name__ == '__main__':
    unittest.main()
