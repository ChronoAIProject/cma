#!/usr/bin/env python3
"""Write and reopen rejected Sandbox controls through the actual application.

An unprovisioned Sandbox cannot pause. Each writer admits a command, observes
the worker's not-eligible terminal receipt, and leaves it for the opposite
reader. Queued reads are progress observations, never compatibility success.
"""
from datetime import datetime
import json
import re
import secrets
import time
import uuid

from migration_browser_fixture import BrowserFixture
from migration_profile_fixture import Fixture, main
from migration_sandbox_fixture import SandboxFixture, STAGES, manifests as base_manifests, require

COLLECTIONS = ['sandbox_controls']
SHAPES = ['rejected-sandbox-control', 'sandbox-control-embedded-admission-audit',
          'sandbox-control-terminal-replay', 'sandbox-control-revision-fence']
IDENTITY = ('command_id', 'sandbox_id', 'command', 'expected_revision',
            'admitted_at', 'queue_deadline')


class ControlBrowserFixture(BrowserFixture):
    defer_pending_callback = True


class ControlProfileFixture(Fixture):
    browser_type = ControlBrowserFixture


def manifests(objects):
    from migration_sandbox_documents import infrastructure
    objects = base_manifests(objects)
    api = next(o for o in objects if o['kind'] == 'Deployment' and o['metadata']['name'] == 'api')
    return infrastructure(objects, api['metadata']['namespace'], secrets.token_urlsafe(24))


def receipt(value, sandbox_id, revision, terminal=False):
    require(isinstance(value, dict) and set(value) - {'lifecycle'} == {*IDENTITY, 'revision', 'state'},
            'control receipt shape differs')
    # Released readers since c6a804151e add an optional public `lifecycle`
    # result (`SandboxCommandLifecycleView`); when present it has exactly that.
    if 'lifecycle' in value:
        lifecycle = value['lifecycle']
        require(isinstance(lifecycle, dict) and set(lifecycle) == {'result'}
                and isinstance(lifecycle['result'], dict), 'control receipt lifecycle differs')
    require(isinstance(value['command_id'], str) and re.fullmatch(r'scmd_[a-f0-9]{32}', value['command_id'])
            and value['sandbox_id'] == sandbox_id and value['command'] == 'pause'
            and type(value['expected_revision']) is int and value['expected_revision'] == revision,
            'control receipt identity differs')
    times = [datetime.fromisoformat(value[key].replace('Z', '+00:00'))
             for key in ('admitted_at', 'queue_deadline')]
    require(all(t.tzinfo is not None for t in times)
            and 0 < (times[1] - times[0]).total_seconds() <= 300, 'control deadline differs')
    state = value['state']
    require(isinstance(state, dict), 'invalid control state')
    if state.get('status') == 'queued' and not terminal:
        require(state == {'status': 'queued'} and type(value['revision']) is int
                and value['revision'] == 1, 'queued control revision differs')
    else:
        require(set(state) == {'status', 'at', 'reason'} and state['status'] == 'rejected'
                and state['reason'] == 'not_eligible' and type(value['revision']) is int
                and value['revision'] == 2, 'control did not reach not-eligible rejection')
        at = datetime.fromisoformat(state['at'].replace('Z', '+00:00'))
        require(at.tzinfo is not None and times[0] <= at < times[1], 'control terminal time differs')
    return value


class SandboxControlFixture(SandboxFixture):
    def unprovisioned_sandbox(self, identifier):
        for attempt in range(31):
            value = self.request('GET', '/api/v1/sandboxes/' + identifier)
            self.stable(value)
            require(value.get('workspaces') == [] and all(value.get(k) == 0 for k in (
                'workspace_count', 'worktree_count', 'agent_count')), 'unexpected native hierarchy')
            if value['status'] == 'failed':
                require(value['revision'] > 1, 'failed Sandbox lacks a stale positive revision')
                return value
            if attempt < 30:
                time.sleep(2)
        raise RuntimeError('sandbox migration: Sandbox did not settle before control admission')

    def terminal(self, admitted):
        path = '/api/v1/sandbox-commands/' + admitted['command_id']
        for attempt in range(31):
            value = receipt(self.request('GET', path), admitted['sandbox_id'], admitted['expected_revision'])
            require(all(value[k] == admitted[k] for k in IDENTITY), 'control identity changed while queued')
            if value['state']['status'] != 'queued':
                return receipt(value, admitted['sandbox_id'], admitted['expected_revision'], True)
            if attempt < 30:
                time.sleep(2)
        raise RuntimeError('sandbox migration: control did not reach terminal within 60 seconds')

    def write_control(self, identifier):
        before = self.unprovisioned_sandbox(identifier)
        route = '/api/v1/sandboxes/' + identifier + '/commands'
        body = {'command': 'pause', 'expected_revision': before['revision']}
        key = 'matrix-control-' + uuid.uuid4().hex
        admitted = receipt(self.request('POST', route, body, key, 202), identifier, before['revision'])
        terminal = self.terminal(admitted)
        return {'route': route, 'body': body, 'key': key, 'admitted': admitted,
                'receipt': terminal, 'sandbox_revision': before['revision'],
                'events': self.events(identifier)}

    def verify_controls(self, commands):
        require(isinstance(commands, list) and len(commands) == 2
                and len({c['receipt']['command_id'] for c in commands}) == 2,
                'control receipts missing or duplicated')
        for command in commands:
            saved = command['receipt']
            receipt(saved, saved['sandbox_id'], command['sandbox_revision'], True)
            path = '/api/v1/sandbox-commands/' + saved['command_id']
            require(self.request('GET', path) == saved, 'retained terminal control changed')
            require(self.request('POST', command['route'], command['body'], command['key'], 202) == saved,
                    'terminal control replay changed')
            # A changed request under the same key must fail without minting
            # another command; a fresh key still obeys the Sandbox revision CAS.
            changed = {**command['body'], 'expected_revision': command['sandbox_revision'] - 1}
            conflict = self.request('POST', command['route'], changed, command['key'], 409)
            require(conflict.get('code') == 'idempotency_key_reused', 'wrong control replay conflict')
            stale = self.request('POST', command['route'], changed,
                                 'matrix-stale-' + uuid.uuid4().hex, 409)
            require(stale.get('code') == 'sandbox_command_conflict', 'stale control revision was accepted')
            require(self.request('GET', path) == saved, 'refused control changed terminal receipt')
            after = self.unprovisioned_sandbox(saved['sandbox_id'])
            require(after['revision'] == command['sandbox_revision']
                    and self.events(saved['sandbox_id']) == command['events'],
                    'rejected control mutated Sandbox lifecycle or history')

    def exercise(self, stage, profiles):
        require(stage in STAGES, 'unknown control fixture stage')
        base = super().exercise(stage, profiles)
        writer = stage if stage in ('old-write', 'new-write') else (
            'old-write' if stage == 'new-read-old-write' else 'new-write')
        path = self.output / (writer + '-sandbox-controls.private.json')
        if writer == stage:
            admitted = json.loads((self.output / (writer + '-sandbox-admission.private.json')).read_text())
            commands = [self.write_control(row['stable']['id']) for row in admitted['rows']]
            self.verify_controls(commands)
            path.write_text(json.dumps({'writer': writer, 'commands': commands}, indent=2) + '\n')
        else:
            saved = json.loads(path.read_text())
            require(saved.get('writer') == writer, 'control writer provenance differs')
            self.verify_controls(saved.get('commands'))
        return {'collections': base['collections'] + COLLECTIONS,
                'shapes': base['shapes'] + SHAPES, 'samples': base['samples'] + 2,
                'checks': {**base['checks'], **{shape: True for shape in SHAPES}}}


if __name__ == '__main__':
    main(SandboxControlFixture, fixture_type=ControlProfileFixture)
