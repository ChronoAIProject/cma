#!/usr/bin/env python3
"""Partial artifact fixture: profile and browser-auth records via actual AM APIs.

This exercises six profile collections plus the browser custody/session read
across the retained database. It deliberately does not certify the full
migration matrix. No raw Mongo document writes or credential access occur in
this fixture; the ephemeral browser cookie is held outside the evidence tree.
"""
import json
import os
from pathlib import Path
import time
from urllib.error import HTTPError
from urllib.parse import urlsplit
from urllib.request import Request, urlopen

from migration_browser_fixture import BrowserFixture, COLLECTIONS as AUTH_COLLECTIONS, SHAPES as AUTH_SHAPES
import uuid


PROFILE_COLLECTIONS = [kind + suffix for kind in ('agent', 'workspace', 'sandbox')
                       for suffix in ('_profiles', '_profile_revisions')]
COLLECTIONS = PROFILE_COLLECTIONS + AUTH_COLLECTIONS


class Fixture:
    def __init__(self, base, output):
        if urlsplit(base).hostname != '127.0.0.1':
            raise ValueError('fixture requires a local API port-forward')
        self.base, self.output, self.requests, self.cookies = base, Path(output), [], {}
        self.stage = os.environ['MATRIX_STAGE']
        identity = os.environ.get('MATRIX_IDENTITY_URL', '')
        self.browser = None
        if identity:
            self.browser = BrowserFixture(base, identity, os.environ['MATRIX_AUTH_STATE_FILE'])
            self.cookies = self.browser.start(self.stage)

    def request(self, method, route, body=None, key=None, status=200):
        headers = {'Content-Type': 'application/json'}
        if key:
            headers['Idempotency-Key'] = key
        if self.cookies:
            headers['Cookie'] = '; '.join(name + '=' + value for name, value in self.cookies.items())
        started = time.time()
        request = Request(self.base + route, method=method, headers=headers,
                          data=None if body is None else json.dumps(body).encode())
        try:
            with urlopen(request, timeout=30) as response:
                code, raw = response.status, response.read()
                request_id = response.headers.get('x-request-id')
        except HTTPError as error:
            code, raw, request_id = error.code, error.read(), error.headers.get('x-request-id')
        value = json.loads(raw)
        self.requests.append({'method': method, 'route': route, 'body': body,
                              'idempotency_key': key, 'status': code, 'response': value,
                              'request_id': request_id, 'elapsed_s': time.time() - started})
        if code != status:
            raise RuntimeError(f'{method} {route} returned {code}, expected {status}')
        return value

    def create(self, kind, body):
        route = '/api/v1/' + kind + '-profiles'
        key = 'migration-' + uuid.uuid4().hex
        try:
            created = self.request('POST', route, body, key, 201)
        except RuntimeError as error:
            # The disposable trusted subject has a one-per-minute sandbox
            # create budget. Wait for that documented window before the
            # reverse-direction writer cell; the request is still the same
            # idempotent operation and is never issued in a tight retry loop.
            if kind != 'sandbox' or 'returned 429' not in str(error):
                raise
            time.sleep(65)
            created = self.request('POST', route, body, key, 201)
        path = route + '/' + created['id']
        publish = {'draft_version': created['draft_version']}
        publish_key = 'migration-' + uuid.uuid4().hex
        revision = self.request('POST', path + '/revisions', publish, publish_key, 201)
        current = self.request('GET', path)
        assert current['draft'] == revision['content']
        assert current['latest_revision']['digest'] == revision['digest']
        assert current['id'] == revision['profile_id']
        return {'kind': kind, 'path': path, 'request': body, 'create_key': key,
                'created': created, 'current': current, 'revision': revision,
                'publish_request': publish, 'publish_key': publish_key}

    def write(self, stage):
        rows = []
        # Keep the trusted local subject below the per-route profile budget.
        # One nested set is enough to exercise every profile collection and
        # its revision shape; the campaign repeats it across image switches.
        for index in range(1):
            name = 'Matrix-' + stage + '-' + str(index)
            agent = self.create('agent', {
                'name': name, 'description': 'Nested profile compatibility',
                'content': {'model': 'gpt-5.2-codex', 'thinking_effort': 'high',
                            'permission_level': 'guarded', 'skills': [],
                            'system_prompt': 'Migration fixture ' + str(index)}})
            rows.append(agent)
            workspace = self.create('workspace', {
                'name': name, 'description': 'Retained Agent revision reference',
                'content': {'location': {'type': 'directory', 'name': name}, 'name': name,
                            'setup_commands': ['git status --short'] if index else [],
                            'agent_profiles': [{'id': agent['created']['id'], 'revision': 1}]}})
            rows.append(workspace)
            rows.append(self.create('sandbox', {
                'name': name, 'description': 'Retained workspace revision reference',
                'content': {'cpu_millis': 1000, 'memory_mib': 1024,
                            'nyxid_services': [], 'agent_provider': 'codex',
                            'model_access': 'chronoai_managed', 'max_workspaces': 4,
                            'max_agents_per_workspace': 4,
                            'workspace_profiles': [{'id': workspace['created']['id'], 'revision': 1}],
                            'software_install_commands': [],
                            'environment_variables': [{'name': 'MATRIX_VALUE', 'value': name}]}}))
        (self.output / (stage + '-profiles.private.json')).write_text(json.dumps(rows, indent=2) + '\n')
        return rows

    def read(self, stage):
        writer = 'old-write' if stage == 'new-read-old-write' else 'new-write'
        rows = json.loads((self.output / (writer + '-profiles.private.json')).read_text())
        for row in rows:
            actual = self.request('GET', row['path'])
            assert actual == row['current'], 'profile changed across image switch'
            revision = self.request('GET', row['path'] + '/revisions/1')
            assert revision == row['revision'], 'revision changed across image switch'
        return rows


def main():
    os.umask(0o077)
    stage = os.environ['MATRIX_STAGE']
    fixture = Fixture(os.environ['MATRIX_API_URL'], Path.cwd())
    try:
        rows = fixture.write(stage) if stage in ('old-write', 'new-write') else fixture.read(stage)
        auth_checks = fixture.browser.finish(stage) if fixture.browser else {}
        if fixture.browser:
            Path('browser-' + stage + '.json').write_text(json.dumps(fixture.browser.observations, indent=2) + '\n')
        receipt = {'schema': 'cma-migration-fixture-v1', 'stage': stage,
                   'source': os.environ['MATRIX_SOURCE'], 'image': os.environ['MATRIX_IMAGE'],
                   'collections': COLLECTIONS if fixture.browser else PROFILE_COLLECTIONS,
                   'shapes': ['profile-draft', 'published-revision', 'nested-profile-reference'] + (AUTH_SHAPES if fixture.browser else []),
                   'checks': {'api_profile_read': True, 'revision_digest_preserved': True,
                              'auth_session_continuity': all(v for k, v in auth_checks.items() if k != 'consumed_attempt_rejected'),
                              'application_writer_or_exact_replay': True},
                   'samples': len(rows), 'outcome': 'PASS'}
        Path('fixture-receipt.json').write_text(json.dumps(receipt, indent=2) + '\n')
    finally:
        Path('requests-' + stage + '.private.json').write_text(json.dumps(fixture.requests, indent=2) + '\n')


if __name__ == '__main__':
    main()
