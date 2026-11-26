#!/usr/bin/env python3
"""Read retained user names before a new callback can rewrite the directory.

The public projection covers users.display_name and its schema-checked lookup.
Avatar, first/last-seen timestamps, directory pagination, and malformed stored
rows require separate fixtures; a passing projection does not cover those cells.
"""
import json
import os
from urllib.parse import quote

from migration_browser_fixture import BrowserFixture, OWNER
from migration_directory_provider import NAMES
from migration_profile_fixture import Fixture, main

COLLECTIONS = ['users']
SHAPES = ['retained-public-directory-name-before-login', 'directory-name-update']
READERS = {'new-read-old-write': 'old-write', 'old-read-new-write': 'new-write'}


def require(condition, message):
    if not condition:
        raise RuntimeError('directory migration: ' + message)


def validate_page(page, record):
    require(isinstance(page, dict) and set(page) == {'items', 'next'}
            and page['next'] is None and isinstance(page['items'], list), 'incomplete profile page')
    rows = page['items']
    require(all(isinstance(row, dict) and isinstance(row.get('id'), str) for row in rows),
            'invalid profile identity')
    ids = [row['id'] for row in rows]
    require(len(ids) == len(set(ids)), 'duplicate profile identity')
    require(all(row.get('owner') == OWNER and row.get('owner_name') == record['name']
                for row in rows), 'retained directory name missing or changed')
    expected = {row['id']: row for row in record['profiles']}
    require(len(expected) == 3, 'expected three distinct profile witnesses')
    found = {row['id']: row for row in rows if row['id'] in expected}
    require(found == expected, 'retained profile witness changed or disappeared')


class DirectoryBrowser(BrowserFixture):
    def control(self, method, suffix):
        bearer = os.environ.get('MATRIX_ADMIN_BEARER')
        require(bool(bearer), 'synthetic control credential missing')
        code, _, body = self.raw(method, self.identity + '/fixture/directory/' + suffix,
                                {'Authorization': 'Bearer ' + bearer})
        require(code == 200, 'synthetic control rejected')
        value = json.loads(body)
        require(isinstance(value, dict) and set(value) == {'profile_stage', 'token_responses'}
                and value['profile_stage'] in NAMES and type(value['token_responses']) is int
                and value['token_responses'] >= 0, 'synthetic control response invalid')
        return value

    def start(self, stage):
        require(stage in NAMES or stage in READERS, 'unknown stage')
        self.directory_verified = False
        output = self.state_file.parent
        if stage in READERS:
            writer = READERS[stage]
            record = json.loads((output / (writer + '-directory.private.json')).read_text())
            require(record.get('writer') == writer and record.get('name') == NAMES[writer],
                    'writer provenance changed')
            # This changes only the provider's display material. The application
            # row must remain unchanged until a later real sign-in callback.
            before = self.control('POST', 'reader')
            require(before['profile_stage'] == 'reader', 'provider replacement missing')
            route = '/api/v1/admin/profiles?owner=' + quote(OWNER, safe='') + '&limit=200'
            status, headers, raw = self.raw('GET', self.api + route,
                {'Authorization': 'Bearer ' + os.environ['MATRIX_ADMIN_BEARER']})
            value = json.loads(raw)
            after = self.control('GET', 'state')
            observation = {'stage': stage, 'writer': writer, 'route': route, 'status': status,
                           'request_id': headers.get('x-request-id'), 'response': value,
                           'provider_before': before, 'provider_after': after}
            (output / ('directory-read-' + stage + '.private.json')).write_text(
                json.dumps(observation, indent=2) + '\n')
            require(status == 200, 'retained directory request failed')
            require(before == after, 'authentication exchanged tokens before retained read')
            validate_page(value, record)
            self.directory_verified = True
        else:
            require(stage in ('old-write', 'new-write'), 'unknown writer stage')
            require(self.control('POST', stage)['profile_stage'] == stage, 'writer profile not selected')
        return super().start(stage)


class DirectoryFixture(Fixture):
    browser_type = DirectoryBrowser


class DirectoryProjection:
    def __init__(self, fixture):
        self.fixture = fixture

    def exercise(self, stage, profiles):
        fixture = self.fixture
        require(isinstance(fixture.browser, DirectoryBrowser), 'directory browser required')
        if stage in ('old-write', 'new-write'):
            page = fixture.admin_request('GET', '/api/v1/admin/profiles?owner=' + quote(OWNER, safe='') + '&limit=200')
            ids = {row['created']['id'] for row in profiles}
            require(isinstance(page, dict) and isinstance(page.get('items'), list)
                    and all(isinstance(row, dict) for row in page['items']), 'profile page missing')
            witnesses = [row for row in page['items'] if row.get('id') in ids]
            record = {'writer': stage, 'name': NAMES[stage], 'profiles': witnesses}
            validate_page(page, record)
            (fixture.output / (stage + '-directory.private.json')).write_text(json.dumps(record, indent=2) + '\n')
        else:
            require(stage in READERS and fixture.browser.directory_verified,
                    'pre-callback retained read did not pass')
        return {'collections': COLLECTIONS, 'shapes': SHAPES, 'samples': 3,
                'checks': {'directory_name_verified': True,
                           'directory_read_precedes_reauthentication': True}}


if __name__ == '__main__':
    main(DirectoryProjection, fixture_type=DirectoryFixture)
