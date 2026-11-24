#!/usr/bin/env python3
"""Persist owner grants, revocations and replay receipts through real CMA APIs.

Use --share-fixture on the kind runner. No runtime assignment is needed. These
selected Sandbox grants do not cover Agent links or recipient authorization.
"""
from datetime import datetime
import json
from pathlib import Path
import re
import uuid

from migration_browser_fixture import OWNER
from migration_profile_fixture import main
from migration_sandbox_fixture import SandboxFixture, STAGES
from migration_share_provider import ORGANIZATION

COLLECTIONS = ['resource_policies', 'resource_shares', 'resource_share_commands']
SHAPES = ['sandbox-organization-share', 'share-mode-revision', 'retained-share-revocation',
          'share-command-exact-replay', 'resource-policy-revision']
# Both selected CMA versions project cookie-authenticated management requests
# through resource_request_context as an API actor without delegation.
ACTOR = {'kind': 'api', 'subject': OWNER, 'delegation_id': None}


def require(condition, message):
    if not condition:
        raise RuntimeError('share migration: ' + message)


def timestamp(value):
    require(isinstance(value, str), 'timestamp missing')
    try:
        parsed = datetime.fromisoformat(value.replace('Z', '+00:00'))
    except ValueError:
        raise RuntimeError('share migration: timestamp invalid') from None
    require(parsed.tzinfo is not None, 'timestamp has no offset')
    return parsed


class ShareFixture:
    def __init__(self, fixture):
        self.fixture, self.output = fixture, Path(fixture.output)

    def request(self, method, route, body=None, key=None, status=200):
        return self.fixture.request(method, route, body=body, key=key, status=status)

    @staticmethod
    def share(value, sandbox, revision, mode, revoked):
        require(isinstance(value, dict) and set(value) == {
            'id', 'owner', 'selector', 'target_org_id', 'mode', 'issuer_actor',
            'created_at', 'revoked_at', 'revision'}, 'share fields changed')
        require(isinstance(value['id'], str) and re.fullmatch(r'shr_[a-f0-9]{32}', value['id']),
                'invalid share ID')
        require(value['owner'] == {'kind': 'person', 'subject': OWNER}
                and value['issuer_actor'] == ACTOR,
                'share attribution changed')
        require(value['selector'] == {'kind': 'sandbox', 'sandbox_id': sandbox}
                and value['target_org_id'] == ORGANIZATION, 'share target changed')
        require(type(value['revision']) is int and value['revision'] == revision
                and value['mode'] == mode, 'share mode or revision changed')
        created = timestamp(value['created_at'])
        if revoked:
            require(timestamp(value['revoked_at']) >= created, 'revocation precedes create')
        else:
            require(value['revoked_at'] is None, 'active share was revoked')

    def receipt(self, value, sandbox, revision, mode, revoked):
        require(isinstance(value, dict) and set(value) == {
            'operation_id', 'share', 'actor', 'at', 'policy_revision'}, 'receipt fields changed')
        require(isinstance(value['operation_id'], str)
                and re.fullmatch(r'[a-f0-9]{64}', value['operation_id']), 'invalid operation ID')
        require(value['actor'] == ACTOR, 'receipt actor changed')
        require(type(value['policy_revision']) is int and value['policy_revision'] == revision,
                'policy revision changed')
        self.share(value['share'], sandbox, revision, mode, revoked)
        require(timestamp(value['at']) >= timestamp(value['share']['created_at']),
                'receipt precedes create')
        if revoked:
            require(value['at'] == value['share']['revoked_at'], 'revocation receipt time differs')

    def command(self, commands, method, route, body, sandbox, revision, mode, revoked=False):
        key = 'matrix-share-' + uuid.uuid4().hex
        value = self.request(method, route, body, key)
        self.receipt(value, sandbox, revision, mode, revoked)
        commands.append({'method': method, 'route': route, 'body': body, 'key': key,
                         'receipt': value})
        return value['share']

    def access(self, sandbox):
        value = self.request('GET', '/api/v1/resource-access/sandbox/' + sandbox)
        require(isinstance(value, dict) and value.get('owner') == {
            'kind': 'person', 'is_current_person': True}, 'owner access changed')
        require(type(value.get('policy_revision')) is int and value['policy_revision'] == 2,
                'policy did not retain both mutations')
        require(isinstance(value.get('capabilities'), list)
                and 'view_details' in value['capabilities'], 'owner lost view capability')
        return value

    def verify(self, record):
        require(len({row['current']['id'] for row in record['rows']}) == 2,
                'duplicate share identity')
        operations = [command['receipt']['operation_id']
                      for row in record['rows'] for command in row['commands']]
        require(len(operations) == 4 and len(set(operations)) == 4,
                'missing or duplicate operation identity')
        for row in record['rows']:
            sandbox, current, commands = row['sandbox'], row['current'], row['commands']
            route = '/api/v1/resource-shares/sandbox/' + sandbox
            item = route + '/' + current['id']
            require(self.request('GET', item) == current, 'retained share changed')
            require(self.access(sandbox) == row['access'], 'retained policy changed')
            active = self.request('GET', route)
            require(active == {'items': [] if current['revoked_at'] else [current]},
                    'active inventory omitted, duplicated or revived a grant')
            # Replay all historical commands, including a create whose grant
            # is now revoked. The returned receipt must remain historical and
            # the live grant must remain at its latest revision.
            for command in commands:
                replay = self.request(command['method'], command['route'], command['body'], command['key'])
                require(replay == command['receipt'], 'immutable command receipt changed')
            create = commands[0]
            changed = {**create['body'], 'mode': 'org_roles' if create['body']['mode'] == 'view' else 'view'}
            refused = self.request('POST', route, changed, create['key'], 409)
            require(refused.get('code') == 'idempotency_key_reused', 'wrong replay refusal')
            for method, target, body in [
                ('PATCH', item, {'expected_revision': 1, 'mode': 'view'}),
                ('DELETE', item + '?expected_revision=1', None),
            ]:
                denied = self.request(method, target, body, 'matrix-stale-' + uuid.uuid4().hex, 409)
                require(denied.get('code') == 'resource_access_changed', 'wrong stale refusal')
            require(self.request('GET', item) == current, 'replay or rejected write changed grant')
            require(self.access(sandbox) == row['access'], 'replay or rejected write changed policy')
            require(self.request('GET', route) == active, 'replay changed active inventory')

    def exercise(self, stage, profiles):
        require(stage in STAGES, 'unknown stage')
        admission = SandboxFixture(self.fixture).exercise(stage, profiles)
        writer = stage if stage in ('old-write', 'new-write') else (
            'old-write' if stage == 'new-read-old-write' else 'new-write')
        path = self.output / (writer + '-resource-shares.private.json')
        if stage == writer:
            parents = json.loads((self.output / (writer + '-sandbox-admission.private.json')).read_text())
            rows = []
            for index, parent in enumerate(parents['rows']):
                sandbox = parent['stable']['id']
                route = '/api/v1/resource-shares/sandbox/' + sandbox
                mode = 'view' if index == 0 else 'org_roles'
                commands = []
                created = self.command(commands, 'POST', route, {'target_org_id': ORGANIZATION,
                    'mode': mode}, sandbox, 1, mode)
                item = route + '/' + created['id']
                current = self.command(commands, 'PATCH' if index == 0 else 'DELETE',
                    item if index == 0 else item + '?expected_revision=1',
                    {'expected_revision': 1, 'mode': 'org_roles'} if index == 0 else None,
                    sandbox, 2, 'org_roles', revoked=index == 1)
                require({k: v for k, v in current.items() if k not in ('mode', 'revision', 'revoked_at')}
                        == {k: v for k, v in created.items() if k not in ('mode', 'revision', 'revoked_at')},
                        'mutation changed share identity')
                rows.append({'sandbox': sandbox, 'current': current, 'commands': commands,
                             'access': self.access(sandbox)})
            record = {'writer': writer, 'rows': rows}
            self.verify(record)
            path.write_text(json.dumps(record, indent=2) + '\n')
        else:
            record = json.loads(path.read_text())
            require(record.get('writer') == writer and len(record.get('rows', [])) == 2,
                    'writer provenance differs')
            self.verify(record)
        return {'collections': admission['collections'] + COLLECTIONS,
                'shapes': admission['shapes'] + SHAPES, 'samples': admission['samples'] + 2,
                'checks': {**admission['checks'], **dict.fromkeys(SHAPES, True)}}


if __name__ == '__main__':
    main(ShareFixture)
