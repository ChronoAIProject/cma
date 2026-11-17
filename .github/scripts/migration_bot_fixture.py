#!/usr/bin/env python3
"""Bot aggregate compatibility through actual API writers and retained readers.

Gateway/key issuance is synthetic and scoped to the disposable identity fixture.
These receipts prove only the listed persisted Bot shapes, never external bot
delivery or every internal BSON variant. No application store is edited directly.
"""
from copy import deepcopy
import json
import os
import re
import time
from urllib.parse import urlsplit
from urllib.request import Request, urlopen
import uuid

from migration_profile_fixture import main

STATES = ('active', 'disabled', 'failed', 'deleted')
STAGES = ('old-write', 'new-read-old-write', 'new-write', 'old-read-new-write')
SHAPES = ['bot-active-with-channel-and-test', 'bot-disabled-after-key-rotation',
          'bot-failed-after-provider-refusal-cleanup', 'bot-deleted-creation-replay',
          'bot-pinned-profile-snapshots', 'bot-creation-fingerprint-conflict']


def require(condition, message):
    if not condition:
        raise RuntimeError('bot migration: ' + message)


class BotFixture:
    def __init__(self, fixture):
        self.fixture, self.request, self.output = fixture, fixture.request, fixture.output

    def dependencies(self):
        base = os.environ.get('MATRIX_IDENTITY_URL', '')
        require(urlsplit(base).hostname == '127.0.0.1', 'dependency must be local')
        require(bool(self.fixture.admin_bearer), 'dependency observer credential missing')
        req = Request(base + '/fixture/bot-state', headers={
            'Authorization': 'Bearer ' + self.fixture.admin_bearer})
        with urlopen(req, timeout=10) as response:
            result = json.load(response)
        require(isinstance(result, dict) and set(result) == {'channels', 'keys', 'effects', 'key_effects'},
                'dependency counters missing')
        require(set(result['effects']) == {'create', 'update', 'delete', 'test'}
                and set(result['key_effects']) == {'mint', 'revoke'}, 'dependency effect counters missing')
        values = [result['channels'], result['keys'], *result['effects'].values(),
                  *result['key_effects'].values()]
        require(all(type(n) is int and n >= 0 for n in values), 'invalid dependency counter')
        return result

    def settle(self, route, state, timeout=180):
        deadline = time.monotonic() + timeout
        while True:
            value = self.request('GET', route, status=(200, 404) if state == 'deleted' else 200)
            if state == 'deleted' and value.get('code') == 'bot_not_found':
                return None
            if value.get('state') == state and value.get('operation') is None:
                return value
            require(time.monotonic() < deadline, 'Bot failed to settle at ' + state)
            time.sleep(2)

    def exercise(self, stage, profiles):
        require(stage in STAGES, 'unsupported stage')
        before = self.dependencies()
        if stage in ('old-write', 'new-write'):
            records = self.write(stage, profiles)
            settled = self.dependencies()
            require(settled['channels'] == before['channels'] + 2
                    and settled['keys'] == before['keys'] + 2, 'resource cleanup mismatch')
            require(settled['effects']['create'] == before['effects']['create'] + 4
                    and settled['effects']['delete'] == before['effects']['delete'] + 2,
                    'creation or cleanup effect count mismatch')
            (self.output / (stage + '-bots.private.json')).write_text(
                json.dumps(records, indent=2) + '\n')
        else:
            writer = 'old-write' if stage == 'new-read-old-write' else 'new-write'
            records = json.loads((self.output / (writer + '-bots.private.json')).read_text())
            settled = before
        require(len(records) == 4 and {r['state'] for r in records} == set(STATES), 'shape set missing')
        for record in records:
            self.verify(record)
        after = self.dependencies()
        require(after == settled, 'reader/replay repeated an external effect')
        (self.output / (stage + '-bot-dependencies.json')).write_text(
            json.dumps({'before': before, 'settled': settled, 'after': after}, indent=2) + '\n')
        return {'collections': ['bots'], 'shapes': SHAPES, 'samples': len(records),
                'checks': {'bot_retained_state_preserved': True,
                           'bot_creation_replay_no_external_effect': True,
                           'bot_failed_and_deleted_resources_cleaned': True,
                           'bot_fingerprint_conflict_refused': True}}

    def write(self, stage, profiles):
        require({row['kind'] for row in profiles} == {'agent', 'workspace', 'sandbox'}, 'profile set')
        refs = {row['kind']: {'id': row['created']['id'], 'revision': row['revision']['revision']}
                for row in profiles}
        records = []
        for state in STATES:
            body = {'name': 'Matrix-' + stage + '-' + state, 'connection_ref': 'matrix-' + stage + '-' + state,
                    'profile': refs['agent'], 'sandbox_profile': refs['sandbox'], 'workspace_profile': refs['workspace'],
                    'admission': {'type': 'scoped', 'senders': {'type': 'open'},
                                  'chats': {'type': 'allowlist', 'ids': ['matrix-chat']}, 'groups': 'deny'},
                    'reply': {'auto_final': True, 'ack_after_ms': 0}}
            key = 'migration-bot-' + uuid.uuid4().hex
            created = self.request('POST', '/api/v1/bots', body, key, 201)
            identifier = created.get('bot_id')
            require(isinstance(identifier, str) and re.fullmatch(r'bot_[a-f0-9]{32}', identifier), 'Bot identity')
            route = '/api/v1/bots/' + identifier
            self.settle(route, 'failed' if state == 'failed' else 'active')
            if state == 'active':
                tested = self.request('POST', route + '/test')
                require(tested.get('first_text') == 'synthetic gateway response', 'test response missing')
            elif state == 'disabled':
                self.request('POST', route + '/disable', status=202)
                self.settle(route, 'disabled')
                self.request('POST', route + '/rotate-keys', status=202)
                self.settle(route, 'disabled')
            elif state == 'deleted':
                self.request('DELETE', route, status=202)
                self.settle(route, 'deleted')
            current = self.request('POST', '/api/v1/bots', body, key, 201)
            require(current.get('bot_id') == identifier and current.get('state') == state
                    and current.get('operation') is None, 'replayed stable state mismatch')
            require(all(current.get(field) == body[field] for field in
                        ('name', 'connection_ref', 'profile', 'sandbox_profile', 'workspace_profile')),
                    'immutable Bot identity or profile reference changed')
            if state in ('active', 'disabled'):
                require(current.get('channel') is not None and current.get('credentials_expire_at'),
                        'channel or credential metadata missing')
            else:
                require(current.get('channel') is None and current.get('credentials_expire_at') is None,
                        'cleanup retained external resources')
            if state == 'failed':
                require(bool(current.get('failure')), 'provider failure missing')
            if state == 'active':
                require(current.get('last_test') == tested, 'test result was not persisted')
            records.append({'state': state, 'route': route, 'body': body, 'key': key, 'current': current})
        return records

    def verify(self, record):
        expected = record['current']
        if record['state'] == 'deleted':
            absent = self.request('GET', record['route'], status=404)
            require(absent.get('code') == 'bot_not_found', 'deleted Bot was readable')
        else:
            require(self.request('GET', record['route']) == expected, 'retained Bot changed')
        replay = self.request('POST', '/api/v1/bots', record['body'], record['key'], 201)
        require(replay == expected, 'creation replay changed retained Bot')
        altered = deepcopy(record['body'])
        altered['name'] += '-conflict'
        conflict = self.request('POST', '/api/v1/bots', altered, record['key'], 409)
        require(conflict.get('code') == 'bot_conflict', 'creation fingerprint conflict not typed')
        require(self.request('POST', '/api/v1/bots', record['body'], record['key'], 201) == expected,
                'conflicting create mutated retained Bot')
        listing = self.request('GET', '/api/v1/bots?limit=200')
        require(isinstance(listing.get('items'), list) and listing.get('next_after') is None, 'incomplete Bot list')
        matches = [row for row in listing['items'] if row.get('bot_id') == expected['bot_id']]
        require(matches == ([] if record['state'] == 'deleted' else [expected]), 'list/read disagreement')


if __name__ == '__main__':
    main(BotFixture)
