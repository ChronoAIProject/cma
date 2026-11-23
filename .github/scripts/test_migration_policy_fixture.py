"""Settings readback must survive pagination without hiding changed receipts."""
import copy
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from urllib.parse import parse_qs, urlsplit

from migration_profile_fixture import Fixture


def activity(revision):
    return {'id': 'fixture-activity-' + str(revision), 'action': 'configure',
            'target': {'kind': 'resource', 'id': 'platform_settings'},
            'outcome': {'status': 'succeeded'},
            'detail': {'setting': 'platform_settings', 'revision': str(revision)}}


class PolicyFixtureTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.fixture = Fixture.__new__(Fixture)
        self.fixture.output = Path(directory.name)
        self.settings = {'revision': 2, 'settings': {
            'usage_reporting': {'measured_visible': False}}}
        self.usage = {'owner': {'sandbox_count': 0}, 'admin': {'sandbox_count': 0},
                      'owner_page': {'items': [], 'next': None},
                      'admin_page': {'items': [], 'next': None}}
        self.expected = {'settings': self.settings, 'usage': self.usage,
                         'activity': [activity(1), activity(2)]}
        for writer in ('old-write', 'new-write'):
            (self.fixture.output / (writer + '-policy.private.json')).write_text(
                json.dumps(self.expected))
        self.pages = [
            {'items': [activity(2)] + [
                {'id': 'fixture-read-' + str(i), 'action': 'read'} for i in range(49)],
             'next': 'older/+ page'},
            {'items': [activity(1)], 'next': None},
        ]
        self.routes = []

    def request(self, method, route, **kwargs):
        self.routes.append((method, route))
        path = urlsplit(route).path
        if path.endswith('/usage'):
            key = 'admin' if '/admin/' in path else 'owner'
            return {**copy.deepcopy(self.usage[key]), 'as_of': 200}
        if path.endswith('/usage/sandboxes'):
            key = 'admin_page' if '/admin/' in path else 'owner_page'
            return {**copy.deepcopy(self.usage[key]), 'as_of': 200}
        if path.endswith('/settings'):
            return copy.deepcopy(self.settings)
        if path.endswith('/activity'):
            query = parse_qs(urlsplit(route).query)
            index = 1 if query.get('cursor') == ['older/+ page'] else 0
            return copy.deepcopy(self.pages[index])
        self.fail('unexpected fixture route')

    def read(self, stage='old-read-new-write'):
        with patch('migration_profile_fixture.time.sleep'), \
                patch.object(self.fixture, 'request', side_effect=self.request), \
                patch.object(self.fixture, 'admin_request', side_effect=self.request):
            return self.fixture.policy_and_usage(stage)

    def test_both_readers_compare_all_records_across_cursor_pages(self):
        for stage in ('old-read-new-write', 'new-read-old-write'):
            with self.subTest(stage=stage):
                self.assertEqual(self.read(stage), self.expected)
        self.assertIn(('GET', '/api/v1/admin/activity?limit=50&cursor=older%2F%2B%20page'),
                      self.routes)

    def test_exhausted_page_without_old_record_is_a_failure(self):
        self.pages[1]['items'] = []
        with self.assertRaisesRegex(RuntimeError, 'activity changed'):
            self.read()

    def test_changed_or_extra_activity_still_fails_after_pagination(self):
        for changed in ({**activity(1), 'outcome': {'status': 'failed'}},
                        {**activity(1), 'actor': 'changed'},
                        {**activity(1), 'detail': {'revision': '99'}}):
            with self.subTest(changed=changed):
                self.pages[1]['items'] = [changed]
                with self.assertRaisesRegex(RuntimeError, 'activity changed'):
                    self.read()
        self.pages[1]['items'] = [activity(1), activity(3)]
        with self.assertRaisesRegex(RuntimeError, 'activity changed'):
            self.read()

    def test_settings_or_usage_semantic_changes_are_not_ignored(self):
        self.settings['settings']['usage_reporting']['measured_visible'] = True
        with self.assertRaisesRegex(RuntimeError, 'settings changed'):
            self.read()
        self.settings['settings']['usage_reporting']['measured_visible'] = False
        self.usage['owner']['sandbox_count'] = 1
        with self.assertRaisesRegex(RuntimeError, 'usage response changed'):
            self.read()

    def test_retained_usage_rows_ignore_only_projected_observation_clocks(self):
        row = {'sandbox_id': 'sbx_fixture', 'status': 'failed', 'created_at': 100,
               'tracking_started_at': 100, 'totals': {'uptime_seconds': 0},
               'allocation': {'running': False}, 'as_of': 200}
        for key in ('owner_page', 'admin_page'):
            self.usage[key]['items'] = [copy.deepcopy(row)]
        expected = copy.deepcopy(self.expected)
        for key in ('owner_page', 'admin_page'):
            expected['usage'][key]['items'][0].pop('as_of')
        (self.fixture.output/'new-write-policy.private.json').write_text(json.dumps(expected))
        self.assertEqual(self.read(), expected)
        for change in ({'sandbox_id': 'changed'}, {'created_at': 101},
                       {'totals': {'uptime_seconds': 1}}, {'allocation': {'running': True}}):
            self.usage['owner_page']['items'][0] = {**row, **change}
            with self.subTest(change=change), self.assertRaisesRegex(RuntimeError, 'usage response changed'):
                self.read()

    def test_malformed_row_clock_is_not_discarded(self):
        for value in (None, True, -1, '200'):
            self.usage['owner_page']['items'] = [{'sandbox_id': 'sbx_fixture', 'as_of': value}]
            with self.subTest(value=value), self.assertRaisesRegex(RuntimeError, 'timestamp'):
                self.read()

    def write(self, *, stale_activity=False, bad_patch=False):
        before = {'revision': 1, 'settings': {
            'usage_reporting': {'measured_visible': True}}}
        self.pages = [{'items': [activity(1)] if stale_activity else
                       [activity(2), activity(1)], 'next': None}]

        def admin(method, route, body=None, status=200):
            if route.endswith('/settings'):
                if method == 'PATCH':
                    self.assertEqual(body, {'expected_revision': 1,
                        'usage_reporting': {'measured_visible': False}})
                    before.update(copy.deepcopy(self.settings))
                    if bad_patch:
                        before['settings']['usage_reporting']['measured_visible'] = True
                return copy.deepcopy(before)
            return self.request(method, route, body=body, status=status)

        with patch.object(self.fixture, 'request', side_effect=self.request), \
                patch.object(self.fixture, 'admin_request', side_effect=admin):
            return self.fixture.policy_and_usage('new-write')

    def test_writer_saves_exact_settings_and_configure_records(self):
        self.assertEqual(self.write(), self.expected)
        saved = json.loads((self.fixture.output / 'new-write-policy.private.json').read_text())
        self.assertEqual(saved, self.expected)

    def test_writer_cannot_count_an_old_configure_record_as_current_write(self):
        with self.assertRaisesRegex(RuntimeError, 'activity receipt'):
            self.write(stale_activity=True)

    def test_writer_rejects_patch_that_did_not_change_the_requested_setting(self):
        with self.assertRaisesRegex(RuntimeError, 'patch did not persist'):
            self.write(bad_patch=True)


if __name__ == '__main__':
    unittest.main()
