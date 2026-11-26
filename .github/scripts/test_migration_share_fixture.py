#!/usr/bin/env python3
"""Fault tests for the grant fixture, not application compatibility evidence."""
from copy import deepcopy
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from urllib.parse import parse_qs, urlsplit

from migration_browser_fixture import OWNER
from migration_share_fixture import ShareFixture, STAGES, timestamp
from test_migration_sandbox_fixture import PROFILES, SandboxAPI


class ShareAPI(SandboxAPI):
    def __init__(self, output):
        super().__init__(output)
        self.shares, self.share_commands, self.policies = {}, {}, {}

    def request(self, method, route, body=None, key=None, status=200):
        if route.startswith('/api/v1/resource-access/sandbox/'):
            sandbox = route.rsplit('/', 1)[1]
            return {'owner': {'kind': 'person', 'is_current_person': True},
                    'capabilities': ['view_details', 'manage_shares'], 'revalidate_at': None,
                    'policy_revision': self.policies.get(sandbox, 0)}
        if not route.startswith('/api/v1/resource-shares/sandbox/'):
            return super().request(method, route, body, key, status)
        parts = urlsplit(route)
        segments = parts.path.strip('/').split('/')
        sandbox = segments[4]
        share_id = segments[5] if len(segments) == 6 else None
        fingerprint = [method, route, body]
        if method == 'GET':
            response = self.shares[share_id] if share_id else {'items': [r for r in self.shares.values()
                if r['selector']['sandbox_id'] == sandbox and not r['revoked_at']]}
            return deepcopy(response)
        if key in self.share_commands:
            saved, receipt = self.share_commands[key]
            code, response = (200, receipt) if fingerprint == saved else (
                409, {'code': 'idempotency_key_reused'})
        elif method != 'POST' and self.shares[share_id]['revision'] != (
                body['expected_revision'] if method == 'PATCH'
                else int(parse_qs(parts.query)['expected_revision'][0])):
            code, response = 409, {'code': 'resource_access_changed'}
        else:
            actor = {'kind': 'api', 'subject': OWNER, 'delegation_id': None}
            now = '2026-01-01T00:00:' + format(len(self.share_commands), '02d') + 'Z'
            if method == 'POST':
                share_id = 'shr_' + format(len(self.shares) + 1, '032x')
                share = {'id': share_id, 'owner': {'kind': 'person', 'subject': OWNER},
                    'selector': {'kind': 'sandbox', 'sandbox_id': sandbox},
                    'target_org_id': body['target_org_id'], 'mode': body['mode'],
                    'issuer_actor': actor, 'created_at': now, 'revoked_at': None, 'revision': 1}
            else:
                share = deepcopy(self.shares[share_id])
                share['revision'] += 1
                if method == 'PATCH':
                    share['mode'] = body['mode']
                else:
                    share['revoked_at'] = now
            self.shares[share_id] = share
            self.policies[sandbox] = self.policies.get(sandbox, 0) + 1
            code, response = 200, {'operation_id': hashlib.sha256(key.encode()).hexdigest(),
                'share': deepcopy(share), 'actor': actor, 'at': now,
                'policy_revision': self.policies[sandbox]}
            self.share_commands[key] = deepcopy((fingerprint, response))
        if code != status:
            raise RuntimeError(f'HTTP {code}, expected {status}')
        return deepcopy(response)


class ShareFixtureTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.output = Path(directory.name)
        self.api = ShareAPI(self.output)
        self.fixture = ShareFixture(self.api)
        self.profiles = deepcopy(PROFILES)

    def writer(self):
        return self.fixture.exercise('old-write', self.profiles)

    def test_four_stages_preserve_live_and_revoked_grants_and_command_receipts(self):
        for stage in STAGES:
            result = self.fixture.exercise(stage, self.profiles)
            self.assertEqual(len(result['collections']), 6)
            self.assertEqual(result['samples'], 4)
            self.assertTrue(all(result['checks'].values()))
        self.assertEqual(len(self.api.shares), 4)
        self.assertEqual(len(self.api.share_commands), 8)
        self.assertEqual(sum(r['revoked_at'] is not None for r in self.api.shares.values()), 2)
        self.assertEqual(set(self.api.policies.values()), {2})

    def test_each_retained_share_field_is_compared(self):
        self.writer()
        identifier = next(iter(self.api.shares))
        original = deepcopy(self.api.shares[identifier])
        for key in original:
            self.api.shares[identifier] = {**original, key: 'corrupt'}
            with self.subTest(key=key), self.assertRaisesRegex(RuntimeError, 'retained share'):
                self.fixture.exercise('new-read-old-write', [])

    def test_each_receipt_field_is_compared_on_replay(self):
        self.writer()
        key = next(iter(self.api.share_commands))
        original = deepcopy(self.api.share_commands[key])
        for field in original[1]:
            self.api.share_commands[key] = (original[0], {**original[1], field: 'corrupt'})
            with self.subTest(field=field), self.assertRaisesRegex(RuntimeError, 'receipt changed'):
                self.fixture.exercise('new-read-old-write', [])

    def test_active_inventory_cannot_omit_duplicate_or_revive_a_share(self):
        self.writer()
        request = self.api.request
        for mode in ('omit', 'duplicate', 'revive'):
            def changed(method, route, **kwargs):
                value = request(method, route, **kwargs)
                if method == 'GET' and route.startswith('/api/v1/resource-shares/') and 'items' in value:
                    if mode == 'omit':
                        value['items'] = []
                    elif mode == 'duplicate':
                        value['items'] *= 2
                    elif not value['items']:
                        value['items'] = [list(self.api.shares.values())[1]]
                return value
            with self.subTest(mode=mode), patch.object(self.api, 'request', side_effect=changed):
                with self.assertRaisesRegex(RuntimeError, 'active inventory'):
                    self.fixture.exercise('new-read-old-write', [])

    def test_no_policy_revision_or_authority_changes_are_ignored(self):
        self.writer()
        request = self.api.request
        for extra in ({'policy_revision': 3}, {'policy_revision': True},
                      {'capabilities': []}, {'capabilities': ['view_details', 'unexpected']},
                      {'owner': {'kind': 'organization', 'organization_id': 'wrong'}}):
            def changed(method, route, **kwargs):
                value = request(method, route, **kwargs)
                return {**value, **extra} if '/resource-access/' in route else value
            with self.subTest(extra=extra), patch.object(self.api, 'request', side_effect=changed):
                with self.assertRaises(RuntimeError):
                    self.fixture.exercise('new-read-old-write', [])

    def test_wrong_refusal_or_a_refused_write_with_side_effect_fails(self):
        self.writer()
        request = self.api.request
        original = deepcopy(self.api.shares)
        for mode in ('wrong_code', 'write_effect', 'replay_effect'):
            self.api.shares = deepcopy(original)
            def changed(method, route, **kwargs):
                value = request(method, route, **kwargs)
                if '/resource-shares/' in route:
                    if kwargs.get('status') == 409:
                        if mode == 'wrong_code':
                            value['code'] = 'unrelated_conflict'
                        elif mode == 'write_effect':
                            next(iter(self.api.shares.values()))['mode'] = 'view'
                    elif method == 'POST' and mode == 'replay_effect':
                        next(iter(self.api.shares.values()))['mode'] = 'view'
                return value
            with self.subTest(mode=mode), patch.object(self.api, 'request', side_effect=changed):
                with self.assertRaises(RuntimeError):
                    self.fixture.exercise('new-read-old-write', [])

    def test_duplicate_effect_from_replay_cannot_hide_behind_original_row(self):
        self.writer()
        request = self.api.request
        def changed(method, route, **kwargs):
            value = request(method, route, **kwargs)
            if method == 'POST' and '/resource-shares/' in route:
                row = deepcopy(next(iter(self.api.shares.values())))
                row['id'] = 'shr_' + 'f' * 32
                self.api.shares[row['id']] = row
            return value
        with patch.object(self.api, 'request', side_effect=changed):
            with self.assertRaisesRegex(RuntimeError, 'replay changed active inventory'):
                self.fixture.exercise('new-read-old-write', [])

    def test_malformed_first_writer_response_does_not_emit_sharing_receipt(self):
        request = self.api.request
        for field, value in [('share', {}), ('operation_id', 'wrong'), ('actor', {}),
                             ('policy_revision', True), ('at', 'bad')]:
            with self.subTest(field=field):
                self.api = ShareAPI(self.output)
                request = self.api.request
                self.fixture = ShareFixture(self.api)
                self.profiles = deepcopy(PROFILES)
                def changed(method, route, **kwargs):
                    response = request(method, route, **kwargs)
                    return {**response, field: value} if '/resource-shares/' in route else response
                with patch.object(self.api, 'request', side_effect=changed):
                    with self.assertRaises(RuntimeError):
                        self.writer()
                self.assertFalse((self.output/'old-write-resource-shares.private.json').exists())

    def test_share_schema_validation_checks_identity_revision_and_clock(self):
        self.writer()
        row = next(iter(self.api.shares.values()))
        sandbox = row['selector']['sandbox_id']
        for field, value in [('id', 'wrong'), ('owner', {}), ('issuer_actor', {}), ('selector', {}),
            ('target_org_id', 'other'), ('mode', 'view'), ('revision', True),
            ('created_at', None), ('created_at', 'bad'), ('created_at', '2026-01-01'),
            ('revoked_at', '2026-01-01T00:00:00Z')]:
            with self.subTest(field=field, value=value), self.assertRaises(RuntimeError):
                self.fixture.share({**row, field: value}, sandbox, 2, 'org_roles', False)
        revoked = list(self.api.shares.values())[1]
        with self.assertRaisesRegex(RuntimeError, 'revocation precedes'):
            self.fixture.share({**revoked, 'revoked_at': '2025-01-01T00:00:00Z'},
                               revoked['selector']['sandbox_id'], 2, 'org_roles', True)
        with self.assertRaises(RuntimeError):
            timestamp(17)

    def test_wrong_stage_provenance_and_unavailable_api_fail_closed(self):
        with self.assertRaisesRegex(RuntimeError, 'unknown stage'):
            self.fixture.exercise('wrong', [])
        with patch.object(self.api, 'request', side_effect=RuntimeError('HTTP 503')):
            with self.assertRaisesRegex(RuntimeError, 'HTTP 503'):
                self.writer()
        self.writer()
        path = self.output/'old-write-resource-shares.private.json'
        original = json.loads(path.read_text())
        for change in ('writer', 'share', 'operation'):
            value = deepcopy(original)
            if change == 'writer':
                value['writer'] = 'new-write'
            elif change == 'share':
                value['rows'][1]['current']['id'] = value['rows'][0]['current']['id']
            else:
                value['rows'][1]['commands'][0]['receipt']['operation_id'] = (
                    value['rows'][0]['commands'][0]['receipt']['operation_id'])
            path.write_text(json.dumps(value))
            with self.subTest(change=change), self.assertRaises(RuntimeError):
                self.fixture.exercise('new-read-old-write', [])


if __name__ == '__main__':
    unittest.main()
