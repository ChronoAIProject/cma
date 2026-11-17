#!/usr/bin/env python3
"""Exercise durable Trigger authoring and completed operation receipts via HTTP.

This extension shares the profile campaign's retained Mongo database. It never
creates a launch or a runtime. Its receipts cover only the named authoring
shapes; pending/unknown-commit operation claims need separate fixtures.
"""
from copy import deepcopy
import json
import re
import uuid

from migration_profile_fixture import main

COLLECTIONS = ['trigger_definitions', 'trigger_definition_revisions', 'operation_claims']
SHAPES = ['trigger-draft', 'trigger-published-revision', 'trigger-retired-definition',
          'trigger-active-and-retired-placements', 'completed-trigger-operation-replay']


def require(condition, message):
    if not condition:
        raise RuntimeError('trigger migration: ' + message)


class TriggerFixture:
    def __init__(self, fixture):
        self.fixture = fixture
        self.request = fixture.request
        self.output = fixture.output

    def exercise(self, stage, profiles):
        require(stage in ('old-write', 'new-write', 'new-read-old-write',
                          'old-read-new-write'), 'unsupported stage')
        if stage in ('old-write', 'new-write'):
            self.prepare_profiles(stage, profiles)
            records = self.write(stage, profiles)
            (self.output / (stage + '-triggers.private.json')).write_text(
                json.dumps(records, indent=2) + '\n')
        else:
            writer = 'old-write' if stage == 'new-read-old-write' else 'new-write'
            records = json.loads((self.output / (writer + '-triggers.private.json')).read_text())
            require(isinstance(records, list) and len(records) == 3,
                    'expected draft, published, and retired records')
            for record in records:
                self.verify(record)
        return {'collections': COLLECTIONS, 'shapes': SHAPES, 'samples': len(records),
                'checks': {'trigger_authoring_readback': True,
                           'trigger_revision_and_placement_preserved': True,
                           'trigger_completed_receipt_replay': True,
                           'trigger_stale_revision_refused': True}}

    def command(self, method, route, body=None, status=200):
        key = 'migration-trigger-' + uuid.uuid4().hex
        response = self.request(method, route, body=body, key=key, status=status)
        return {'method': method, 'route': route, 'body': body, 'key': key,
                'status': status, 'response': response}

    def prepare_profiles(self, stage, profiles):
        require({row['kind'] for row in profiles} == {'agent', 'workspace', 'sandbox'},
                'all profile references required')
        # Published Triggers require public, pinned profiles. Record this real
        # authoring transition before the next reader compares profile state.
        for row in profiles:
            changed = self.command('PUT', row['path'] + '/visibility', {'visibility': 'public'})
            current = self.request('GET', row['path'])
            require(current['id'] == row['created']['id'] and current['visibility'] == 'public',
                    'profile visibility was not published')
            require(changed['response'] == current, 'profile visibility readback mismatch')
            row['current'] = current
        (self.output / (stage + '-profiles.private.json')).write_text(
            json.dumps(profiles, indent=2) + '\n')

    def write(self, stage, profiles):
        target = {row['kind'] + '_profile':
                  {'id': row['created']['id'], 'revision': row['revision']['revision']}
                  for row in profiles}
        records = []
        for state in ('draft', 'published', 'retired'):
            content = {
                'presentation': {'title': 'Matrix-' + stage + '-' + state,
                                 'description': 'Retained authoring fixture',
                                 'button_label': 'open_in_cma', 'theme': 'auto',
                                 'signup_hint': 'Local compatibility check'},
                'target': target,
                'reuse': {'sandbox': 'always_new', 'workspace': 'always_new', 'agent': 'ask'},
                'first_message': 'Retained instruction; this fixture never launches it.',
                'consent': {'always_review': True},
            }
            create = self.command('POST', '/api/v1/triggers', {'content': content}, 201)
            created = create['response']
            require(isinstance(created, dict) and isinstance(created.get('id'), str)
                    and re.fullmatch(r'[A-Za-z0-9_-]{1,128}', created['id']), 'invalid Trigger ID')
            require(created['draft'] == content and created['draft_version'] == 1
                    and created['availability'] == 'draft', 'draft creation changed content')
            route = '/api/v1/triggers/' + created['id']
            commands = [create]
            for label in ('active', 'retired'):
                placement = self.command('POST', route + '/placements',
                                         {'source_url': 'https://matrix.invalid/' + label,
                                          'label': label}, 201)
                commands.append(placement)
                if label == 'retired':
                    commands.append(self.command('POST', route + '/placements/' +
                                                 placement['response']['id'] + '/retire'))
            public_route = None
            if state != 'draft':
                published = self.command('POST', route + '/publish', {'draft_version': 1}, 201)
                commands.append(published)
                revision = published['response']
                require(revision['trigger_id'] == created['id'] and revision['revision'] == 1
                        and revision['content'] == content, 'publication changed content')
                public_route = '/api/v1/triggers/public/' + revision['public_id']
                if state == 'retired':
                    commands.append(self.command('POST', route + '/retire'))
            observed = self.snapshot(route, public_route)
            require(observed['detail']['availability'] == state, 'availability mismatch')
            require(len(observed['revisions']) == (0 if state == 'draft' else 1),
                    'unexpected revision count')
            require(len(observed['placements']) == 2 and len(observed['active_placements']) == 1,
                    'placement retention mismatch')
            record = {'route': route, 'public_route': public_route, 'commands': commands,
                      'observed': observed}
            self.verify(record)
            records.append(record)
        return records

    def snapshot(self, route, public_route):
        result = {
            'detail': self.request('GET', route),
            'revisions': self.request('GET', route + '/revisions'),
            'placements': self.request('GET', route + '/placements?include_retired=true'),
            'active_placements': self.request('GET', route + '/placements'),
        }
        if public_route:
            result['preview'] = self.request('GET', public_route, use_cookies=False)
        return result

    def verify(self, record):
        route, public_route = record['route'], record['public_route']
        expected = record['observed']
        require(self.snapshot(route, public_route) == expected,
                'retained definition, revision, placement, or preview changed')
        for command in record['commands']:
            actual = self.request(command['method'], command['route'], body=command['body'],
                                  key=command['key'], status=command['status'])
            require(actual == command['response'], 'completed operation replay changed')
        if expected['detail']['availability'] == 'draft':
            before = expected['detail']
            altered = deepcopy(before['draft'])
            altered['presentation']['title'] += '-stale'
            self.command('PUT', route, {'draft_version': before['draft_version'] + 1,
                                       'content': altered}, 409)
        require(self.snapshot(route, public_route) == expected,
                'replay or stale update mutated retained data')


if __name__ == '__main__':
    main(TriggerFixture)
