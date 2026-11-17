#!/usr/bin/env python3
"""Exercise Event Worker Profile revisions through the public API.

The fixture deliberately uses the same profile references that the base
campaign created. It checks current and immutable revision documents in both
reader directions, including update and retirement replay semantics. It does
not write Mongo directly and does not claim native worker delivery coverage.
"""
import json
from pathlib import Path
import re

from migration_profile_fixture import main


COLLECTIONS = ['event_worker_profiles', 'event_worker_profile_revisions']
SHAPES = [
    'event-worker-current-revision',
    'event-worker-immutable-revision-history',
    'event-worker-pinned-profile-references',
    'event-worker-retired-current',
    'event-worker-creation-replay',
]


def require(condition, message):
    if not condition:
        raise RuntimeError('Event Worker migration: ' + message)


class EventWorkerFixture:
    def __init__(self, fixture):
        self.fixture = fixture
        self.request = fixture.request
        self.output = Path(fixture.output)

    @staticmethod
    def _refs(profiles):
        require({row['kind'] for row in profiles} == {'agent', 'workspace', 'sandbox'},
                'Event Worker profile references are incomplete')
        return {
            row['kind'] + '_profile': {
                'id': row['created']['id'],
                'revision': row['revision']['revision'],
            }
            for row in profiles
        }

    @staticmethod
    def _assert_view(value, expected_revision, retired):
        require(isinstance(value, dict), 'Event Worker response is not an object')
        required = {'id', 'owner', 'revision', 'source', 'profile', 'digest',
                    'created_at', 'updated_at', 'retired_at'}
        require(set(value) == required, 'Event Worker response shape changed')
        require(re.fullmatch(r'ewp_[a-f0-9]{32}', value['id']) is not None,
                'Event Worker identity is invalid')
        require(value['revision'] == expected_revision, 'Event Worker revision mismatch')
        require((value['retired_at'] is not None) is retired, 'retirement state mismatch')
        require(re.fullmatch(r'sha256:[a-f0-9]{64}', value['digest']) is not None,
                'Event Worker digest is invalid')
        source = value['source']
        require(set(source) == {'name', 'description', 'agent_profile',
                                'workspace_profile', 'sandbox_profile'},
                'Event Worker source shape changed')

    def _write(self, stage, refs):
        source = {
            'name': 'Matrix Event Worker',
            'description': 'retained profile compatibility fixture',
            **refs,
        }
        key = 'migration-event-worker-' + stage
        created = self.request('POST', '/api/v1/event-worker-profiles', source, key, 201)
        self._assert_view(created, 1, False)
        path = '/api/v1/event-worker-profiles/' + created['id']
        current = self.request('GET', path)
        require(current == created, 'created Event Worker changed on read')

        changed_source = {**source, 'description': 'updated retained description'}
        changed = self.request('PUT', path, {
            'expected_revision': 1,
            'source': changed_source,
        })
        self._assert_view(changed, 2, False)
        revision1 = self.request('GET', path + '/revisions/1')
        self._assert_view(revision1, 1, False)
        require(revision1['source'] == source, 'revision 1 was rewritten')
        revision2 = self.request('GET', path + '/revisions/2')
        require(revision2 == changed, 'revision 2 does not match current update')
        update_replay = self.request('PUT', path, {
            'expected_revision': 1,
            'source': changed_source,
        })
        require(update_replay == changed, 'exact update replay changed revision state')

        retired = self.request('DELETE', path + '?expected_revision=2')
        self._assert_view(retired, 3, True)
        revision3 = self.request('GET', path + '/revisions/3')
        require(revision3 == retired, 'retired revision was not retained')

        creation_replay = self.request('POST', '/api/v1/event-worker-profiles', source, key, 201)
        require(creation_replay == created and creation_replay['revision'] == 1,
                'creation replay did not return immutable revision 1')
        retire_replay = self.request('DELETE', path + '?expected_revision=2')
        require(retire_replay == retired, 'exact retirement replay changed state')
        listed = self.request('GET', '/api/v1/event-worker-profiles?limit=200')
        require(isinstance(listed.get('items'), list) and listed.get('next_after') is None,
                'Event Worker list pagination contract changed')
        require(all(row['id'] != created['id'] for row in listed['items']),
                'retired Event Worker remained in active list')
        return {
            'id': created['id'], 'path': path, 'key': key, 'source': source,
            'changed_source': changed_source, 'created': created,
            'changed': changed, 'retired': retired,
        }

    def _read(self, writer, expected):
        path = expected['path']
        current = self.request('GET', path)
        require(current == expected['retired'], 'current retired Event Worker changed')
        for revision, saved in ((1, expected['created']), (2, expected['changed']),
                                (3, expected['retired'])):
            actual = self.request('GET', path + '/revisions/' + str(revision))
            require(actual == saved, 'immutable Event Worker revision changed')
        replay = self.request('POST', '/api/v1/event-worker-profiles', expected['source'],
                              expected['key'], 201)
        require(replay == expected['created'], 'reader replay changed creation state')
        listed = self.request('GET', '/api/v1/event-worker-profiles?limit=200')
        require(all(row['id'] != expected['id'] for row in listed['items']),
                'reader listed retired Event Worker')
        return {'writer': writer, 'id': expected['id'], 'revisions': 3}

    def exercise(self, stage, profiles):
        require(stage in ('old-write', 'new-read-old-write', 'new-write',
                          'old-read-new-write'), 'unsupported Event Worker stage')
        refs = self._refs(profiles)
        if stage in ('old-write', 'new-write'):
            record = self._write(stage, refs)
            (self.output / (stage + '-event-workers.private.json')).write_text(
                json.dumps(record, indent=2) + '\n')
            checks = {
                'current_and_revision_reads': True,
                'retirement_is_durable': True,
                'creation_and_update_replay': True,
                'retired_list_exclusion': True,
            }
        else:
            writer = 'old-write' if stage == 'new-read-old-write' else 'new-write'
            record = json.loads((self.output / (writer + '-event-workers.private.json')).read_text())
            self._read(writer, record)
            checks = {
                'current_and_revision_reads': True,
                'retirement_is_durable': True,
                'creation_and_update_replay': True,
                'retired_list_exclusion': True,
            }
        return {'collections': COLLECTIONS, 'shapes': SHAPES, 'samples': 3,
                'checks': checks}


if __name__ == '__main__':
    main(EventWorkerFixture)
