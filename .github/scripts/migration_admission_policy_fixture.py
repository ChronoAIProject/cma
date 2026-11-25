#!/usr/bin/env python3
"""Read Lifecycle admission after an application Always-on write and rollback.

Use the sandbox-admission manifest: runtime mutations are denied. A cold
Lifecycle request still admits its parent before runtime readiness. Only the
13b writer changes policy, so a rollback read tests the retained original
admission identity. Any refusal remains a failed compatibility cell.
"""
import json
from pathlib import Path
import re
import time
import uuid

from migration_browser_fixture import OWNER
from migration_profile_fixture import main

STAGES = ('old-write', 'new-read-old-write', 'new-write', 'old-read-new-write')
IMMUTABLE = ('id', 'owner', 'name', 'profile', 'limits', 'created_at')


def require(condition, message):
    if not condition:
        raise RuntimeError('admission policy migration: ' + message)


class AdmissionPolicyFixture:
    def __init__(self, fixture):
        self.fixture = fixture
        self.output = Path(fixture.output)

    def inventory(self):
        page = self.fixture.request('GET', '/api/v1/sandboxes?limit=200')
        require(isinstance(page, dict) and isinstance(page.get('items'), list)
                and page.get('next') is None, 'bounded inventory is incomplete')
        rows = page['items']
        require(all(isinstance(row, dict) and isinstance(row.get('id'), str)
                    for row in rows), 'invalid inventory identity')
        require(len({row['id'] for row in rows}) == len(rows), 'duplicate Sandbox')
        return rows

    @staticmethod
    def stable(value):
        require(isinstance(value, dict) and all(key in value for key in IMMUTABLE),
                'Sandbox view is incomplete')
        require(re.fullmatch(r'sbx_[a-f0-9]{32}', value['id']) is not None
                and value['owner'] == OWNER, 'Sandbox identity differs')
        require(type(value.get('revision')) is int and value['revision'] > 0,
                'invalid revision')
        require(value.get('status') in ('provisioning', 'failed')
                and not value.get('runtime_version'), 'unexpected runtime advancement')
        return {key: value[key] for key in IMMUTABLE}

    def admitted_parent(self, stage):
        before = {row['id'] for row in self.inventory()}
        body = {'name': 'Matrix admission policy ' + stage}
        key = 'matrix-policy-' + uuid.uuid4().hex
        accepted = self.fixture.request('POST', '/api/v1/agents', body, key, 202)
        require(isinstance(accepted, dict) and isinstance(accepted.get('id'), str)
                and accepted['id'] and isinstance(accepted.get('status'), str),
                'lifecycle admission receipt missing')
        for attempt in range(31):
            added = [row for row in self.inventory() if row['id'] not in before]
            require(len(added) <= 1, 'ambiguous Lifecycle parent')
            if added:
                current = self.fixture.request('GET', '/api/v1/sandboxes/' + added[0]['id'])
                self.stable(current)
                require(current['name'].endswith(' sandbox ' + accepted['id']),
                        'parent is not bound to the accepted Lifecycle operation')
                require('always_on' not in current, 'initial policy is not unset')
                return {'operation': accepted, 'request': body, 'key': key,
                        'stable': self.stable(current), 'revision': current['revision'],
                        'always_on': None}
            if attempt < 30:
                time.sleep(2)
        raise RuntimeError('admission policy migration: Lifecycle parent not admitted within 60s')

    def verify(self, record):
        path = '/api/v1/sandboxes/' + record['stable']['id']
        current = self.fixture.request('GET', path)
        require(self.stable(current) == record['stable'], 'retained parent changed')
        require(current['revision'] >= record['revision'], 'revision rewound')
        require(current.get('always_on') is record['always_on'], 'current policy changed')
        listed = [row for row in self.inventory() if row['id'] == current['id']]
        require(len(listed) == 1 and listed[0].get('always_on') is record['always_on'],
                'inventory lost the retained policy')
        operation = self.fixture.request('GET', '/api/v1/agent-lifecycle/' + record['operation']['id'])
        require(operation.get('id') == record['operation']['id'], 'operation identity changed')

    def exercise(self, stage, profiles):
        require(stage in STAGES, 'unknown stage')
        writer = stage if stage in ('old-write', 'new-write') else (
            'old-write' if stage == 'new-read-old-write' else 'new-write')
        path = self.output / (writer + '-admission-policy.private.json')
        if stage == writer:
            record = self.admitted_parent(stage)
            record['writer'] = writer
            if stage == 'new-write':
                updated = self.fixture.admin_request('POST', '/api/v1/admin/sandboxes/'
                    + record['stable']['id'] + '/always-on',
                    {'expected_revision': record['revision'], 'always_on': True})
                require(self.stable(updated) == record['stable']
                        and updated['revision'] == record['revision'] + 1
                        and updated.get('always_on') is True, 'policy write did not persist')
                record.update(revision=updated['revision'], always_on=True)
            path.write_text(json.dumps(record, indent=2) + '\n')
        else:
            record = json.loads(path.read_text())
            require(record.get('writer') == writer, 'writer provenance differs')
        self.verify(record)
        return {'collections': ['sandboxes', 'agent_lifecycle_operations'],
                'shapes': ['lifecycle-admission-parent', 'retained-admission-current-policy'],
                'samples': 1, 'checks': {'parent_identity_preserved': True,
                                        'current_policy_readable': True}}


if __name__ == '__main__':
    main(AdmissionPolicyFixture)
