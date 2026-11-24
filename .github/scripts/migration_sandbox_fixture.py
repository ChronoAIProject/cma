#!/usr/bin/env python3
"""Retain admitted Sandbox records, events and colours across AM images.

The opt-in manifest removes runtime mutation authority. This exercises durable
admission and unprovisioned records, not a successful runtime or Agent journey.
"""
import copy
import json
from pathlib import Path
import re
from urllib.parse import urlencode
import uuid

from migration_browser_fixture import OWNER
from migration_profile_fixture import main

COLLECTIONS = ['sandboxes', 'sandbox_events', 'sandbox_colors']
SHAPES = ['unprovisioned-sandbox', 'sandbox-created-event', 'owner-colour-palette',
          'sandbox-admission-replay']
STAGES = ('old-write', 'new-read-old-write', 'new-write', 'old-read-new-write')
IMMUTABLE = ('id', 'owner', 'name', 'profile', 'limits', 'created_at', 'color', 'nyxid_services')


def require(condition, message):
    if not condition:
        raise RuntimeError('sandbox migration: ' + message)


def manifests(objects):
    """An unavailable runtime dependency must never acquire native authority."""
    result = copy.deepcopy(objects)
    selected = {(obj['kind'], obj['metadata']['name']): obj for obj in result}
    config = selected[('ConfigMap', 'api-env')]['data']
    require(config.get('CHRONO_AUTH_MODE') == 'provider', 'provider auth required')
    for name in ('CHRONO_NATIVE_PREPARED_TARGET', 'CHRONO_NATIVE_MACHINE_PREPARED_TARGET',
                 'CHRONO_NATIVE_WARM_POOL_TARGET', 'CHRONO_NATIVE_MACHINE_WARM_POOL_TARGET'):
        require(config.get(name) == '0', 'runtime pools must be disabled')
    deployments = {name for kind, name in selected if kind == 'Deployment'}
    require(deployments == {'api', 'mongo', 'identity'}, 'unexpected controller deployment')
    roles = [obj for obj in result if obj['kind'] in ('Role', 'ClusterRole')]
    require(len(roles) == 1 and roles[0]['metadata']['name'] == 'cma-runtime-sandbox-reclaimer',
            'unexpected runtime authority')
    bindings = [obj for obj in result if obj['kind'] in ('RoleBinding', 'ClusterRoleBinding')]
    require(len(bindings) == 1 and bindings[0]['kind'] == 'RoleBinding'
            and bindings[0]['roleRef']['kind'] == 'Role'
            and bindings[0]['roleRef']['name'] == roles[0]['metadata']['name'],
            'unexpected runtime authority binding')
    for rule in roles[0]['rules']:
        require(all('/' not in resource and resource != '*' for resource in rule['resources']),
                'subresource or wildcard authority is forbidden')
        rule['verbs'] = ['get', 'list']
    return result


class SandboxFixture:
    def __init__(self, fixture):
        self.fixture, self.output = fixture, Path(fixture.output)

    def request(self, method, route, body=None, key=None, status=200):
        return self.fixture.request(method, route, body=body, key=key, status=status)

    @staticmethod
    def stable(value):
        require(isinstance(value, dict) and all(key in value for key in IMMUTABLE),
                'missing retained Sandbox fields')
        require(re.fullmatch(r'sbx_[a-f0-9]{32}', value['id']) and value['owner'] == OWNER,
                'wrong Sandbox identity')
        require(re.fullmatch(r'#[0-9a-f]{6}', value['color']), 'colour assignment missing')
        require(type(value.get('revision')) is int and value['revision'] >= 1,
                'invalid Sandbox revision')
        require(value.get('status') in ('provisioning', 'failed'),
                'unexpected runtime lifecycle advancement')
        require(not value.get('runtime_version'), 'runtime unexpectedly provisioned')
        return {key: value[key] for key in IMMUTABLE}

    def events(self, identifier):
        rows, cursor = [], 0
        for _ in range(10):
            page = self.request('GET', '/api/v1/sandboxes/' + identifier
                                + '/events?limit=1&after=' + str(cursor))
            require(isinstance(page, list) and len(page) <= 1, 'invalid event page')
            if not page:
                require(rows and rows[0]['seq'] == 1 and rows[0]['kind'] == {'type': 'created'},
                        'admission event missing')
                require(sum(row['kind'] == {'type': 'created'} for row in rows) == 1,
                        'duplicate admission event')
                return rows
            row = page[0]
            require(set(row) == {'sandbox_id', 'seq', 'appended_at', 'kind'}
                    and row['sandbox_id'] == identifier and type(row['seq']) is int
                    and row['seq'] == cursor + 1 and bool(row['appended_at']),
                    'event identity, order or timestamp differs')
            require(isinstance(row['kind'], dict), 'invalid event kind')
            rows.append(row)
            cursor = row['seq']
        raise RuntimeError('sandbox migration: event page bound exceeded')

    def list_ids(self):
        ids, cursor = set(), None
        for _ in range(10):
            query = {'limit': 1}
            if cursor:
                query['cursor'] = cursor
            page = self.request('GET', '/api/v1/sandboxes?' + urlencode(query))
            require(isinstance(page.get('items'), list) and len(page['items']) <= 1,
                    'invalid Sandbox inventory page')
            for row in page['items']:
                require(row['id'] not in ids, 'duplicate inventory entry')
                ids.add(row['id'])
            next_cursor = page.get('next')
            if not next_cursor:
                return ids
            require(page['items'] and next_cursor != cursor, 'inventory cursor made no progress')
            cursor = next_cursor
        raise RuntimeError('sandbox migration: inventory page bound exceeded')

    def verify(self, record):
        for saved in record['rows']:
            actual = self.request('GET', '/api/v1/sandboxes/' + saved['stable']['id'])
            require(self.stable(actual) == saved['stable'], 'retained admission or palette changed')
            require(actual['revision'] >= saved['revision'], 'Sandbox revision rewound')
            require(actual.get('workspaces') == [] and all(actual.get(key) == 0 for key in (
                'workspace_count', 'worktree_count', 'agent_count')), 'unexpected native hierarchy')
            events = self.events(actual['id'])
            require(events[:len(saved['events'])] == saved['events'], 'retained event history changed')
            replay = self.request('POST', '/api/v1/sandboxes', saved['body'], saved['key'], 201)
            require(replay == saved['created'], 'completed admission replay changed')
            changed = {**saved['body'], 'name': saved['body']['name'] + '-changed'}
            refused = self.request('POST', '/api/v1/sandboxes', changed, saved['key'], 422)
            require(refused.get('code') == 'idempotency_key_reused', 'wrong replay conflict')
            after = self.request('GET', '/api/v1/sandboxes/' + actual['id'])
            require(self.stable(after) == saved['stable'], 'rejected mutation changed admission')
        inventory = self.list_ids()
        require({row['stable']['id'] for row in record['rows']} <= inventory,
                'admitted Sandbox missing from inventory')

    def retain_profile_usage(self, stage, profiles, profile):
        # Admission adds two references after the base fixture saved this view.
        # Preserve the post-write count exactly for the opposite image reader;
        # no other profile field may change as a side effect of admission.
        before = profile['current']
        require(type(before.get('uses_count')) is int, 'profile usage count missing')
        expected = {**before, 'uses_count': before['uses_count'] + 2}
        after = self.request('GET', profile['path'])
        require(after == expected, 'admission changed unexpected profile fields or usage count')
        profile['current'] = after
        (self.output / (stage + '-profiles.private.json')).write_text(
            json.dumps(profiles, indent=2) + '\n')

    def exercise(self, stage, profiles):
        require(stage in STAGES, 'unknown stage')
        writer = stage if stage in ('old-write', 'new-write') else (
            'old-write' if stage == 'new-read-old-write' else 'new-write')
        path = self.output / (writer + '-sandbox-admission.private.json')
        if stage == writer:
            profile = next(row for row in profiles if row['path'].startswith('/api/v1/sandbox-profiles/'))
            rows = []
            for index in range(2):
                body = {'sandbox_profile': {'id': profile['created']['id'], 'revision': 1},
                        'name': 'matrix-admission-' + stage + '-' + str(index)}
                key = 'matrix-admission-' + uuid.uuid4().hex
                created = self.request('POST', '/api/v1/sandboxes', body, key, 201)
                stable = self.stable(created)
                require(stable['profile'] == body['sandbox_profile']
                        and stable['name'] == body['name'], 'wrong admitted profile or name')
                rows.append({'body': body, 'key': key, 'created': created, 'stable': stable,
                             'revision': created['revision'], 'events': self.events(created['id'])})
            require(len({row['stable']['color'] for row in rows}) == 2, 'colour collision')
            record = {'writer': writer, 'rows': rows}
            self.verify(record)
            self.retain_profile_usage(stage, profiles, profile)
            path.write_text(json.dumps(record, indent=2) + '\n')
        else:
            record = json.loads(path.read_text())
            require(record.get('writer') == writer and len(record.get('rows', [])) == 2,
                    'writer provenance differs')
            self.verify(record)
        return {'collections': list(COLLECTIONS), 'shapes': list(SHAPES), 'samples': 2,
                'checks': {shape: True for shape in SHAPES}}


if __name__ == '__main__':
    main(SandboxFixture)
