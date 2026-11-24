#!/usr/bin/env python3
"""Regression tests for the public Sandbox control compatibility extension."""
from copy import deepcopy
import json
import tempfile
import unittest
from unittest.mock import patch
from migration_kind_manifests import manifests as base_manifests
import secrets

from migration_sandbox_fixture import STAGES
from migration_sandbox_control_fixture import ControlProfileFixture, SandboxControlFixture, manifests, receipt
from test_migration_sandbox_fixture import PROFILES, SandboxAPI


class ControlAPI(SandboxAPI):
    def __init__(self, output):
        super().__init__(output)
        self.control_rows, self.control_keys = {}, {}

    def request(self, method, route, body=None, key=None, status=200):
        if method == 'POST' and route.endswith('/commands'):
            sandbox_id = route.split('/')[4]
            if body.get('expected_revision') == 0:
                code, value = 400, {'code': 'invalid_sandbox_command'}
            elif key in self.control_keys:
                saved = self.control_keys[key]
                code, value = ((202, deepcopy(saved['response'])) if saved['body'] == body else
                               (409, {'code': 'idempotency_key_reused'}))
            elif body['expected_revision'] != self.rows[sandbox_id]['revision']:
                code, value = 409, {'code': 'sandbox_command_conflict'}
            else:
                command_id = 'scmd_' + format(len(self.control_rows) + 1, '032x')
                value = {'command_id': command_id, 'sandbox_id': sandbox_id, 'command': 'pause',
                         'expected_revision': body['expected_revision'],
                         'admitted_at': '2026-01-01T00:00:00Z',
                         'queue_deadline': '2026-01-01T00:05:00Z', 'revision': 2,
                         'state': {'status': 'rejected', 'at': '2026-01-01T00:00:01Z',
                                   'reason': 'not_eligible'}}
                self.control_rows[command_id] = deepcopy(value)
                self.control_keys[key] = {'body': deepcopy(body), 'response': deepcopy(value)}
                code = 202
        elif method == 'GET' and route.startswith('/api/v1/sandbox-commands/'):
            code, value = 200, deepcopy(self.control_rows[route.rsplit('/', 1)[-1]])
        else:
            value = super().request(method, route, body, key, status)
            if method == 'POST' and status == 201:
                self.rows[value['id']].update(status='failed', revision=3)
            return value
        if code != status:
            raise RuntimeError(f'HTTP {code}, expected {status}')
        return value


class SandboxControlFixtureTests(unittest.TestCase):
    def test_control_profile_keeps_original_sign_in_for_command_replay(self):
        from migration_profile_fixture import Fixture
        self.assertTrue(ControlProfileFixture.browser_type.defer_pending_callback)
        self.assertFalse(Fixture.browser_type.defer_pending_callback)

    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.api = ControlAPI(self.directory.name)
        self.fixture = SandboxControlFixture(self.api)
        self.profiles = deepcopy(PROFILES)

    def writer(self):
        self.fixture.exercise('old-write', self.profiles)
        return json.loads((self.fixture.output / 'old-write-sandbox-controls.private.json').read_text())

    def test_all_reader_directions_retain_rejected_controls(self):
        with tempfile.TemporaryDirectory() as temp:
            api = ControlAPI(temp)
            fixture = SandboxControlFixture(api)
            profiles = deepcopy(PROFILES)
            for stage in STAGES:
                result = fixture.exercise(stage, profiles)
                self.assertEqual(result['collections'], ['sandboxes', 'sandbox_events', 'sandbox_colors',
                                                         'sandbox_controls'])
                self.assertTrue(all(result['checks'].values()))
            self.assertEqual(len(api.control_rows), 4)

    def test_receipt_rejects_unknown_or_mutated_shape(self):
        value = {'command_id': 'scmd_' + 'a' * 32, 'sandbox_id': 'sbx_' + 'b' * 32,
                 'command': 'pause', 'expected_revision': 3,
                 'admitted_at': '2026-01-01T00:00:00Z', 'queue_deadline': '2026-01-01T00:05:00Z',
                 'revision': 2, 'state': {'status': 'rejected', 'at': '2026-01-01T00:00:01Z',
                                           'reason': 'not_eligible'}}
        receipt(value, value['sandbox_id'], 3, True)
        for fault in ({**value, 'command': 'revive'},
                      {**value, 'state': {'status': 'applied'}},
                      {**value, 'revision': 1},
                      {**value, 'unknown': True},
                      {**value, 'lifecycle': None},
                      {**value, 'lifecycle': {'result': 'queued'}},
                      {**value, 'lifecycle': {'result': {}, 'extra': 1}},
                      {**value, 'lifecycle': {}}):
            with self.subTest(fault=fault), self.assertRaises(RuntimeError):
                receipt(fault, value['sandbox_id'], 3, True)

    def test_released_readers_optional_lifecycle_result_is_accepted(self):
        # v0.20.0 and v0.22.0 serialize SandboxCommandReceiptView.lifecycle.
        value = {'command_id': 'scmd_' + 'a' * 32, 'sandbox_id': 'sbx_' + 'b' * 32,
                 'command': 'pause', 'expected_revision': 3,
                 'admitted_at': '2026-01-01T00:00:00Z', 'queue_deadline': '2026-01-01T00:05:00Z',
                 'revision': 2, 'state': {'status': 'rejected', 'at': '2026-01-01T00:00:01Z',
                                           'reason': 'not_eligible'},
                 'lifecycle': {'result': {'status': 'rejected'}}}
        self.assertIs(receipt(value, value['sandbox_id'], 3, True), value)

    def test_terminal_progress_is_bounded_and_preserves_identity(self):
        saved = self.writer()['commands'][0]['receipt']
        queued = {**saved, 'revision': 1, 'state': {'status': 'queued'}}
        with patch.object(self.fixture, 'request', side_effect=[queued, saved]), patch('migration_sandbox_control_fixture.time.sleep') as sleep:
            self.assertEqual(self.fixture.terminal(queued), saved)
            sleep.assert_called_once_with(2)
        with patch.object(self.fixture, 'request', return_value=queued), patch('migration_sandbox_control_fixture.time.sleep') as sleep:
            with self.assertRaisesRegex(RuntimeError, '60 seconds'): self.fixture.terminal(queued)
            self.assertEqual(sleep.call_count, 30)
        changed = {**queued, 'command_id': 'scmd_' + 'f' * 32}
        with patch.object(self.fixture, 'request', return_value=changed), self.assertRaisesRegex(RuntimeError, 'identity changed'):
            self.fixture.terminal(queued)

    def test_sandbox_settlement_is_bounded_and_requires_no_native_hierarchy(self):
        saved = self.writer()['commands'][0]['receipt']
        route = '/api/v1/sandboxes/' + saved['sandbox_id']
        actual = self.api.request('GET', route)
        pending = {**actual, 'status': 'provisioning'}
        with patch.object(self.fixture, 'request', side_effect=[pending, actual]), patch('migration_sandbox_control_fixture.time.sleep') as sleep:
            self.assertEqual(self.fixture.unprovisioned_sandbox(saved['sandbox_id']), actual)
            sleep.assert_called_once_with(2)
        with patch.object(self.fixture, 'request', return_value=pending), patch('migration_sandbox_control_fixture.time.sleep'):
            with self.assertRaisesRegex(RuntimeError, 'did not settle'):
                self.fixture.unprovisioned_sandbox(saved['sandbox_id'])
        for bad in ({**actual, 'agent_count': 1}, {**actual, 'revision': 1}, {**actual, 'status': 'active'}):
            with patch.object(self.fixture, 'request', return_value=bad), self.assertRaises(RuntimeError):
                self.fixture.unprovisioned_sandbox(saved['sandbox_id'])

    def test_reader_rejects_rewritten_terminal_and_missing_or_wrong_writer(self):
        commands = self.writer()['commands']
        self.api.control_rows[commands[0]['receipt']['command_id']]['queue_deadline'] = 'changed'
        with self.assertRaisesRegex(RuntimeError, 'terminal control changed'):
            self.fixture.verify_controls(commands)
        for bad in ([], [commands[0], commands[0]], None):
            with self.assertRaises(RuntimeError): self.fixture.verify_controls(bad)
        with self.assertRaises(RuntimeError): self.fixture.exercise('invalid', [])
        path = self.fixture.output / 'old-write-sandbox-controls.private.json'
        path.write_text(json.dumps({'writer': 'foreign', 'commands': commands}))
        with self.assertRaisesRegex(RuntimeError, 'provenance'):
            self.fixture.exercise('new-read-old-write', [])

    def test_replay_mismatch_wrong_problem_and_mutating_rejection_fail(self):
        commands = self.writer()['commands']
        original = self.fixture.request
        for fault in ('replay', 'problem', 'revision', 'events'):
            def broken(method, route, body=None, key=None, status=200):
                value = original(method, route, body, key, status)
                if fault == 'replay' and status == 202: value['revision'] = 99
                if fault == 'problem' and status == 409: value['code'] = 'unexpected'
                if fault == 'revision' and method == 'GET' and '/sandboxes/' in route and '/events?' not in route:
                    value['revision'] += 1
                if fault == 'events' and '/events?' in route and value:
                    value[0]['appended_at'] = 'changed'
                return value
            with self.subTest(fault=fault), patch.object(self.fixture, 'request', side_effect=broken), self.assertRaises(RuntimeError):
                self.fixture.verify_controls(commands)

    def test_overlay_preserves_runtime_boundary_and_adds_find_only_collector(self):
        credentials = {k: secrets.token_urlsafe(24) for k in (
            'client_id', 'client_secret', 'bearer', 'admin_bearer', 'outsider_bearer', 'refresh_token')}
        credentials['expires_at'] = 2000000000
        objects = base_manifests('api', 'runtime', 'admin', 'key', 'mongo', {},
                                 identity_credentials=credentials)
        original = deepcopy(objects)
        rendered = manifests(objects)
        self.assertEqual(objects, original)
        self.assertFalse(any(o['kind'] == 'Secret' for o in rendered))
        roles = [o for o in rendered if o['kind'] == 'Role']
        self.assertTrue(all(r['verbs'] == ['get', 'list'] for role in roles for r in role['rules']))
        init = next(o for o in rendered if o['kind'] == 'ConfigMap' and o['metadata']['name'] == 'mongo-init')
        self.assertIn('matrix_sandbox_command_reader', init['data']['init.sh'])


if __name__ == '__main__':
    unittest.main()
