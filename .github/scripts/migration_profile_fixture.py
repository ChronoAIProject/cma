#!/usr/bin/env python3
"""Partial artifact fixture: profile and browser-auth records via actual AM APIs.

This exercises profiles, browser custody, settings and activity through the
retained database. Usage calls are route checks without a persisted usage
fixture and do not certify usage collection compatibility or the full
migration matrix. No raw Mongo document writes or credential access occur in
this fixture; the ephemeral browser cookie stays in the private run directory
with mode 0600 and is removed by the runner's cleanup.
"""
import hashlib
import json
import os
from pathlib import Path
import time
from urllib.error import HTTPError
from urllib.parse import quote, urlsplit
from urllib.request import Request, urlopen

from migration_browser_fixture import BrowserFixture, COLLECTIONS as AUTH_COLLECTIONS, SHAPES as AUTH_SHAPES
import migration_profile_contention as contention
import uuid


PROFILE_COLLECTIONS = [kind + suffix for kind in ('agent', 'workspace', 'sandbox')
                       for suffix in ('_profiles', '_profile_revisions')]
COLLECTIONS = PROFILE_COLLECTIONS + AUTH_COLLECTIONS
CREATE_INTERVAL_SECONDS = 6.25


def retained_activity(row):
    record = dict(row)
    if 'actor_reference' in record:
        actor = record.get('actor')
        reference = record.pop('actor_reference')
        if not isinstance(actor, str) or reference != 'actor-' + hashlib.sha256(
                b'cma:operator-activity:actor:v1\0' + actor.encode()).hexdigest()[:16]:
            raise RuntimeError('platform settings activity changed across image switch')
    return record


def usage_snapshot(value, page=False):
    """Exclude only the observation clocks projected by SandboxUsageView.

    Persisted timestamps, allocation, identity and totals still compare exactly.
    Raw HTTP receipts retain the clocks for separate timing inspection.
    """
    if not isinstance(value, dict) or type(value.get('as_of')) is not int or value['as_of'] < 0:
        raise RuntimeError('invalid usage observation timestamp')
    result = {key: field for key, field in value.items() if key != 'as_of'}
    if page:
        if not isinstance(value.get('items'), list):
            raise RuntimeError('invalid usage page')
        result['items'] = [usage_snapshot(row) for row in value['items']]
    return result


class RateLimited(RuntimeError):
    """An explicit rate-limit failure for disposable fixture traffic."""

    def __init__(self, method, route, retry_after):
        super().__init__(f'{method} {route} returned 429, expected a successful response')
        self.retry_after = retry_after


class Fixture:
    browser_type = BrowserFixture

    def __init__(self, base, output):
        if urlsplit(base).hostname != '127.0.0.1':
            raise ValueError('fixture requires a local API port-forward')
        self.base, self.output, self.requests, self.cookies = base, Path(output), [], {}
        self.last_retry_after = None
        self.last_create_request_at = None
        self.stage = os.environ['MATRIX_STAGE']
        # The runner creates this bearer only for the disposable provider
        # fixture.  It is passed through the environment rather than written
        # to a receipt or request log.
        self.admin_bearer = os.environ.get('MATRIX_ADMIN_BEARER')
        identity = os.environ.get('MATRIX_IDENTITY_URL', '')
        self.browser = None
        if identity:
            self.browser = self.browser_type(base, identity, os.environ['MATRIX_AUTH_STATE_FILE'])
            self.cookies = self.browser.start(self.stage)

    def request(self, method, route, body=None, key=None, status=200, bearer=None,
                use_cookies=True):
        if method == 'POST':
            self._pace_create()
        headers = {'Content-Type': 'application/json'}
        if key:
            headers['Idempotency-Key'] = key
        if use_cookies and self.cookies:
            headers['Cookie'] = '; '.join(name + '=' + value for name, value in self.cookies.items())
        if bearer:
            headers['Authorization'] = 'Bearer ' + bearer
        started = time.time()
        request = Request(self.base + route, method=method, headers=headers,
                          data=None if body is None else json.dumps(body).encode())
        try:
            with urlopen(request, timeout=30) as response:
                code, raw = response.status, response.read()
                request_id = response.headers.get('x-request-id')
                retry_after = response.headers.get('Retry-After')
        except HTTPError as error:
            code, raw, request_id = error.code, error.read(), error.headers.get('x-request-id')
            retry_after = error.headers.get('Retry-After')
        try:
            self.last_retry_after = max(0, int(retry_after)) if retry_after else None
        except (TypeError, ValueError):
            self.last_retry_after = None
        if code == 204:
            if raw:
                raise RuntimeError('HTTP 204 response unexpectedly contained a body')
            value = None
        else:
            value = json.loads(raw)
        finished = time.time()
        self.requests.append({'method': method, 'route': route, 'body': body,
                              'idempotency_key': key, 'status': code, 'response': value,
                              'request_id': request_id, 'started_at': started,
                              'finished_at': finished, 'elapsed_s': finished - started})
        expected_codes = status if isinstance(status, (tuple, list, set)) else (status,)
        if code not in expected_codes:
            if code == 429:
                raise RateLimited(method, route, self.last_retry_after)
            raise RuntimeError(f'{method} {route} returned {code}, expected {expected_codes}')
        return value

    def _pace_create(self):
        """Leave room for the manifest's ten-per-minute operation budget."""
        now = time.monotonic()
        if self.last_create_request_at is not None:
            delay = CREATE_INTERVAL_SECONDS - (now - self.last_create_request_at)
            if delay > 0:
                time.sleep(delay)
        self.last_create_request_at = time.monotonic()

    def admin_request(self, method, route, body=None, status=200):
        if not self.admin_bearer:
            raise RuntimeError('admin fixture bearer is unavailable')
        return self.request(method, route, body=body, status=status,
                            bearer=self.admin_bearer, use_cookies=False)

    def admin_activity(self):
        """Read every activity page before comparing durable configure rows.

        The route emits ordinary activity reads as well as configuration
        writes.  A single page can therefore hide an older configure receipt;
        following the cursor keeps the compatibility assertion about persisted
        records instead of the page size.
        """
        rows = []
        cursor = None
        seen_cursors, seen_ids = set(), set()
        for _ in range(20):
            route = '/api/v1/admin/activity?limit=50'
            if cursor:
                route += '&cursor=' + quote(cursor, safe='')
            page = self.admin_request('GET', route)
            if (not isinstance(page, dict) or 'next' not in page
                    or not isinstance(page.get('items'), list)
                    or len(page['items']) > 50):
                raise RuntimeError('admin activity page contract mismatch')
            items = page['items']
            for item in items:
                if (not isinstance(item, dict) or not isinstance(item.get('id'), str)
                        or not item['id'] or item['id'] in seen_ids):
                    raise RuntimeError('admin activity row identity invalid or repeated')
                seen_ids.add(item['id'])
            rows.extend(items)
            cursor = page['next']
            if cursor is None:
                return rows
            if (not isinstance(cursor, str) or not cursor or len(cursor) > 4096
                    or cursor in seen_cursors):
                raise RuntimeError('admin activity cursor invalid or repeated')
            seen_cursors.add(cursor)
            if len(seen_cursors) < 20:
                time.sleep(2)
        raise RuntimeError('admin activity pagination exceeded bounded pages')

    def create(self, kind, body):
        route = '/api/v1/' + kind + '-profiles'
        key = 'migration-' + uuid.uuid4().hex
        created = self.request('POST', route, body, key, 201)
        path = route + '/' + created['id']
        before = self.request('GET', path)
        edited, edit_receipt = contention.race(self, kind, path, before)
        publish = {'draft_version': edited['draft_version']}
        publish_key = 'migration-' + uuid.uuid4().hex
        revision = self.request('POST', path + '/revisions', publish, publish_key, 201)
        current = self.request('GET', path)
        assert current['draft'] == revision['content']
        assert current['latest_revision']['digest'] == revision['digest']
        assert current['id'] == revision['profile_id']
        return {'kind': kind, 'path': path, 'request': body, 'create_key': key,
                'created': created, 'current': current, 'revision': revision,
                'publish_request': publish, 'publish_key': publish_key,
                'edit_receipt': edit_receipt}

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
            contention.verify(self, row['edit_receipt'], actual)
            assert self.request('GET', row['path'] + '/revisions/1') == row['revision'], \
                'update replay changed the published revision'
        return rows

    def policy_and_usage(self, stage):
        """Exercise persisted policy and separately check usage route responses.

        Settings are written only in the disposable kind database.  The
        opposite reader must observe the exact persisted settings response;
        usage responses contain an observation timestamp, so their returned
        counters are compared without it. Empty usage responses do not prove
        compatibility of stored usage documents.
        """
        owner_usage = self.request('GET', '/api/v1/usage')
        admin_usage = self.admin_request('GET', '/api/v1/admin/usage')
        usage_page = self.request('GET', '/api/v1/usage/sandboxes')
        admin_page = self.admin_request('GET', '/api/v1/admin/usage/sandboxes')
        usage = {
            'owner': usage_snapshot(owner_usage),
            'admin': usage_snapshot(admin_usage),
            'owner_page': usage_snapshot(usage_page, page=True),
            'admin_page': usage_snapshot(admin_page, page=True),
        }
        if stage in ('old-write', 'new-write'):
            before = self.admin_request('GET', '/api/v1/admin/settings')
            expected = before['revision']
            visible = not before['settings']['usage_reporting']['measured_visible']
            changed = self.admin_request(
                'PATCH', '/api/v1/admin/settings',
                {'expected_revision': expected,
                 'usage_reporting': {'measured_visible': visible}},
            )
            after = self.admin_request('GET', '/api/v1/admin/settings')
            if (after != changed or after['revision'] != expected + 1
                    or after['settings']['usage_reporting']['measured_visible'] is not visible):
                raise RuntimeError('settings patch did not persist through admin route')
            settings = after
            activity = [item for item in self.admin_activity()
                        if item.get('action') == 'configure'
                        and item.get('target') == {'kind': 'resource', 'id': 'platform_settings'}]
            current = [item for item in activity
                       if item.get('detail', {}).get('revision') == str(after['revision'])
                       and item.get('outcome') == {'status': 'succeeded'}]
            if len(current) != 1:
                raise RuntimeError('settings write did not leave an activity receipt')
            activity.sort(key=lambda item: item['id'])
            (self.output / (stage + '-policy.private.json')).write_text(
                json.dumps({'settings': settings, 'usage': usage,
                            'activity': activity}, indent=2) + '\n')
        else:
            writer = 'old-write' if stage == 'new-read-old-write' else 'new-write'
            expected = json.loads((self.output / (writer + '-policy.private.json')).read_text())
            settings = self.admin_request('GET', '/api/v1/admin/settings')
            if settings != expected['settings']:
                raise RuntimeError('platform settings changed across image switch')
            if usage != expected['usage']:
                raise RuntimeError('usage response changed across image switch')
            activity = [item for item in self.admin_activity()
                        if item.get('action') == 'configure'
                        and item.get('target') == {'kind': 'resource', 'id': 'platform_settings'}]
            activity.sort(key=lambda item: item['id'])
            if [retained_activity(row) for row in activity] != [
                    retained_activity(row) for row in sorted(expected['activity'], key=lambda item: item['id'])]:
                raise RuntimeError('platform settings activity changed across image switch')
        return {'settings': settings, 'usage': usage, 'activity': activity}


def main(extension=None, fixture_type=Fixture):
    os.umask(0o077)
    stage = os.environ['MATRIX_STAGE']
    fixture = fixture_type(os.environ['MATRIX_API_URL'], Path.cwd())
    try:
        rows = fixture.write(stage) if stage in ('old-write', 'new-write') else fixture.read(stage)
        extra = extension(fixture).exercise(stage, rows) if extension else None
        policy = fixture.policy_and_usage(stage)
        auth_checks = fixture.browser.finish(stage) if fixture.browser else {}
        if fixture.browser:
            Path('browser-' + stage + '.json').write_text(json.dumps(fixture.browser.observations, indent=2) + '\n')
        receipt = {'schema': 'cma-migration-fixture-v1', 'stage': stage,
                   'source': os.environ['MATRIX_SOURCE'], 'image': os.environ['MATRIX_IMAGE'],
                   'collections': COLLECTIONS + ['platform_settings', 'activity'] if fixture.browser else PROFILE_COLLECTIONS,
                   'shapes': ['profile-draft', 'published-revision', 'nested-profile-reference',
                              'platform-settings-patch', 'platform-settings-activity',
                              'owner-and-admin-usage-read'] + (AUTH_SHAPES if fixture.browser else []),
                   'checks': {'api_profile_read': True, 'revision_digest_preserved': True,
                              'auth_session_continuity': all(v for k, v in auth_checks.items() if k != 'consumed_attempt_rejected'),
                              'platform_settings_round_trip': bool(policy['settings']),
                              'platform_settings_activity': bool(policy['activity']),
                              'usage_owner_admin_read': bool(policy['usage']),
                              'application_writer_or_exact_replay': True},
                   'samples': len(rows), 'outcome': 'PASS'}
        receipt['collections'] = sorted(set(receipt['collections'] + ['operation_claims']))
        receipt['shapes'].extend(contention.SHAPES)
        receipt['checks'].update(profile_concurrent_version_fence=True,
                                 profile_completed_update_replay=True)
        if extra is not None:
            receipt['collections'] = sorted(set(receipt['collections'] + extra['collections']))
            receipt['shapes'].extend(extra['shapes'])
            receipt['checks'].update(extra['checks'])
            receipt['samples'] += extra['samples']
        Path('fixture-receipt.json').write_text(json.dumps(receipt, indent=2) + '\n')
    finally:
        Path('requests-' + stage + '.private.json').write_text(json.dumps(fixture.requests, indent=2) + '\n')


if __name__ == '__main__':
    main()
