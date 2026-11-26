"""A directory read must fail before authentication can recreate a missing row."""
from copy import deepcopy
from email.message import Message
import json
import os
from pathlib import Path
import secrets
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from migration_browser_fixture import BrowserFixture, OWNER
from migration_directory_fixture import (DirectoryBrowser, DirectoryFixture, DirectoryProjection,
                                         READERS, validate_page)
from migration_directory_provider import NAMES


def record(writer):
    return {'writer': writer, 'name': NAMES[writer], 'profiles': [
        {'id': kind + '-fixture', 'kind': kind, 'owner': OWNER, 'owner_name': NAMES[writer],
         'name': 'Witness ' + kind, 'visibility': 'private', 'updated_at': '2026-11-17T00:00:00Z',
         'retired_at': None} for kind in ('agent', 'workspace', 'sandbox')]}


class DirectoryFixtureTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.output = Path(temporary.name)
        self.environment = patch.dict(os.environ, {'MATRIX_ADMIN_BEARER': secrets.token_urlsafe(32)})
        self.environment.start()
        self.addCleanup(self.environment.stop)

    def browser(self, writer='old-write'):
        self.saved = record(writer)
        (self.output / (writer + '-directory.private.json')).write_text(json.dumps(self.saved))
        self.page = {'items': deepcopy(self.saved['profiles']), 'next': None}
        self.calls = []
        self.state = {'profile_stage': 'reader', 'token_responses': 2}
        self.status = 200
        browser = DirectoryBrowser('http://127.0.0.1:32101', 'http://127.0.0.1:32102',
                                   self.output / 'browser.ephemeral')

        def raw(method, url, headers=None):
            self.calls.append((method, url))
            self.assertEqual(headers, {'Authorization': 'Bearer ' + os.environ['MATRIX_ADMIN_BEARER']})
            if '/api/v1/admin/profiles?' in url:
                response = Message()
                response['x-request-id'] = 'directory-read'
                return self.status, response, json.dumps(self.page).encode()
            if method == 'POST':
                self.state['profile_stage'] = url.rsplit('/', 1)[1]
            return 200, {}, json.dumps(self.state).encode()
        browser.raw = raw
        return browser

    def test_both_directions_read_before_callback_can_rewrite_name(self):
        for stage, writer in READERS.items():
            with self.subTest(stage=stage):
                browser = self.browser(writer)

                def callback(_browser, actual_stage):
                    self.assertEqual(actual_stage, stage)
                    self.assertTrue(browser.directory_verified)
                    self.assertIn('/api/v1/admin/profiles?', self.calls[1][1])
                    self.page['items'][0]['owner_name'] = NAMES['reader']
                    self.state['token_responses'] += 1
                    return {'cma_session': 'synthetic-session'}

                with patch.object(BrowserFixture, 'start', callback):
                    browser.start(stage)
                observation = json.loads((self.output / ('directory-read-' + stage + '.private.json')).read_text())
                self.assertEqual(observation['response']['items'], self.saved['profiles'])
                self.assertEqual(observation['provider_before'], observation['provider_after'])
                self.assertEqual(observation['request_id'], 'directory-read')
                self.assertNotIn(os.environ['MATRIX_ADMIN_BEARER'], json.dumps(observation))
                fixture = SimpleNamespace(browser=browser)
                self.assertEqual(DirectoryProjection(fixture).exercise(stage, [])['collections'], ['users'])

    def test_missing_or_poisoned_name_cannot_be_recreated_by_callback(self):
        for name in (None, '', NAMES['reader'], NAMES['new-write']):
            with self.subTest(name=name):
                browser = self.browser()
                self.page['items'][0]['owner_name'] = name
                with patch.object(BrowserFixture, 'start') as callback:
                    with self.assertRaisesRegex(RuntimeError, 'name missing or changed'):
                        browser.start('new-read-old-write')
                    callback.assert_not_called()

    def test_profile_witness_loss_duplicates_changes_and_incomplete_pages_fail(self):
        mutations = [lambda p: p['items'].pop(),
                     lambda p: p['items'].append(deepcopy(p['items'][0])),
                     lambda p: p['items'][0].update(owner='foreign'),
                     lambda p: p['items'][0].update(name='mutated'),
                     lambda p: p.update(next='hidden-page'),
                     lambda p: p['items'].append(None),
                     lambda p: p.pop('next')]
        for mutate in mutations:
            browser = self.browser()
            mutate(self.page)
            with patch.object(BrowserFixture, 'start') as callback:
                with self.assertRaises(RuntimeError):
                    browser.start('new-read-old-write')
                callback.assert_not_called()

    def test_token_exchange_during_pre_read_and_http_failure_are_fatal(self):
        browser = self.browser()
        original = browser.raw

        def reauthenticate(method, url, headers=None):
            if '/admin/profiles?' in url:
                self.state['token_responses'] += 1
            return original(method, url, headers)
        browser.raw = reauthenticate
        with patch.object(BrowserFixture, 'start') as callback:
            with self.assertRaisesRegex(RuntimeError, 'exchanged tokens'):
                browser.start('new-read-old-write')
            callback.assert_not_called()
        browser = self.browser()
        self.status = 503
        with self.assertRaisesRegex(RuntimeError, 'request failed'):
            browser.start('new-read-old-write')

    def test_writer_profile_selected_before_login_and_saved_for_other_reader(self):
        self.assertIs(DirectoryFixture.browser_type, DirectoryBrowser)
        for writer in ('old-write', 'new-write'):
            browser = self.browser(writer)
            with patch.object(BrowserFixture, 'start', return_value={}) as callback:
                browser.start(writer)
                callback.assert_called_once_with(writer)
                self.assertEqual(self.state['profile_stage'], writer)
            fixture = SimpleNamespace(browser=browser, output=self.output,
                                      admin_request=lambda *_: self.page)
            profiles = [{'created': {'id': row['id']}} for row in self.saved['profiles']]
            result = DirectoryProjection(fixture).exercise(writer, profiles)
            saved = json.loads((self.output / (writer + '-directory.private.json')).read_text())
            self.assertEqual(saved, self.saved)
            self.assertTrue(all(result['checks'].values()))

    def test_missing_pre_read_wrong_browser_and_wrong_provenance_refuse(self):
        browser = self.browser()
        browser.directory_verified = False
        with self.assertRaisesRegex(RuntimeError, 'pre-callback'):
            DirectoryProjection(SimpleNamespace(browser=browser)).exercise('new-read-old-write', [])
        with self.assertRaisesRegex(RuntimeError, 'directory browser'):
            DirectoryProjection(SimpleNamespace(browser=None)).exercise('old-write', [])
        self.saved['writer'] = 'new-write'
        (self.output / 'old-write-directory.private.json').write_text(json.dumps(self.saved))
        with self.assertRaisesRegex(RuntimeError, 'provenance'):
            browser.start('new-read-old-write')
        with self.assertRaisesRegex(RuntimeError, 'unknown stage'):
            browser.start('unexpected')

    def test_control_authority_and_response_are_checked(self):
        browser = self.browser()
        with patch.dict(os.environ, {'MATRIX_ADMIN_BEARER': ''}):
            with self.assertRaisesRegex(RuntimeError, 'credential missing'):
                browser.control('GET', 'state')
        for status, value in [(403, {}), (200, {'profile_stage': 'reader', 'token_responses': True}),
                              (200, {'profile_stage': 'foreign', 'token_responses': 2})]:
            browser.raw = lambda *_, code=status, body=value: (code, {}, json.dumps(body).encode())
            with self.assertRaises(RuntimeError):
                browser.control('GET', 'state')

    def test_three_distinct_witnesses_required(self):
        saved = record('old-write')
        saved['profiles'] = saved['profiles'][:2]
        with self.assertRaisesRegex(RuntimeError, 'three distinct'):
            validate_page({'items': saved['profiles'], 'next': None}, saved)


if __name__ == '__main__':
    unittest.main()
