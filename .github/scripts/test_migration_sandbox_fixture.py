#!/usr/bin/env python3
"""Fault regressions for retained, deliberately unprovisioned Sandboxes."""
from copy import deepcopy
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from urllib.parse import parse_qs, urlsplit

from migration_sandbox_fixture import SandboxFixture, STAGES, manifests
from migration_browser_fixture import OWNER

PROFILES = [{'path': '/api/v1/sandbox-profiles/profile-a', 'created': {'id': 'profile-a'},
             'current': {'id': 'profile-a', 'uses_count': 0, 'draft_version': 2}}]


class SandboxAPI:
    def __init__(self, output):
        self.output, self.rows, self.commands, self.events = Path(output), {}, {}, {}

    def request(self, method, route, body=None, key=None, status=200):
        if method == 'GET' and route == PROFILES[0]['path']:
            return {**PROFILES[0]['current'], 'uses_count': len(self.rows)}
        parsed = urlsplit(route)
        query = parse_qs(parsed.query)
        identifier = parsed.path.split('/')[4] if parsed.path.count('/') > 3 else None
        if method == 'POST':
            if key in self.commands:
                saved = self.commands[key]
                code, response = (201, saved['result']) if saved['body'] == body else (
                    422, {'code': 'idempotency_key_reused'})
            else:
                index = len(self.rows) + 1
                identifier = 'sbx_' + format(index, '032x')
                row = {'id': identifier, 'owner': OWNER, 'name': body['name'],
                       'profile': body['sandbox_profile'], 'limits': {'max_workspaces': 4,
                       'max_agents_per_workspace': 4}, 'created_at': '2026-01-01T00:00:00Z',
                       'color': '#' + format(index, '06x'), 'nyxid_services': [],
                       'status': 'provisioning', 'revision': 1}
                self.rows[identifier] = deepcopy(row)
                self.commands[key] = {'body': deepcopy(body), 'result': deepcopy(row)}
                self.events[identifier] = [{'sandbox_id': identifier, 'seq': 1,
                    'appended_at': row['created_at'], 'kind': {'type': 'created'}}]
                code, response = 201, row
        elif parsed.path.endswith('/events'):
            after = int(query['after'][0])
            code, response = 200, [r for r in self.events[identifier] if r['seq'] > after][:1]
        elif identifier:
            row = self.rows[identifier]
            code, response = 200, {**row, 'workspaces': [], 'workspace_count': 0,
                                   'worktree_count': 0, 'agent_count': 0}
        else:
            ids = sorted(self.rows)
            offset = int(query.get('cursor', ['0'])[0])
            page = ids[offset:offset+1]
            code, response = 200, {'items': [self.rows[i] for i in page]}
            if offset + 1 < len(ids):
                response['next'] = str(offset + 1)
        if code != status:
            raise RuntimeError(f'HTTP {code}, expected {status}')
        return deepcopy(response)


class SandboxFixtureTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.output = Path(self.directory.name)
        self.api = SandboxAPI(self.output)
        self.fixture = SandboxFixture(self.api)
        self.profiles = deepcopy(PROFILES)

    def writer(self):
        self.fixture.exercise('old-write', self.profiles)

    def test_all_four_stages_preserve_admissions_events_palette_and_replay(self):
        for stage in STAGES:
            result = self.fixture.exercise(stage, self.profiles)
            self.assertEqual(result['samples'], 2)
            self.assertEqual(len(result['collections']), 3)
            self.assertTrue(all(result['checks'].values()))
        self.assertEqual(len(self.api.rows), 4)
        self.assertEqual(len(self.api.commands), 4)

    def test_capture_post_admission_usage_for_opposite_reader(self):
        (self.output/'old-write-profiles.private.json').write_text(json.dumps(self.profiles))
        self.writer()
        saved = json.loads((self.output/'old-write-profiles.private.json').read_text())
        self.assertEqual(saved[0]['current']['uses_count'], 2)
        self.assertEqual(saved[0]['current']['draft_version'], 2)

    def test_do_not_accept_unrelated_profile_change_or_wrong_use_count(self):
        request = self.api.request
        for change in ({'uses_count': 1}, {'draft_version': 3}):
            self.api.rows.clear()
            self.api.commands.clear()
            self.profiles = deepcopy(PROFILES)
            def changed(method, route, **kwargs):
                value = request(method, route, **kwargs)
                return {**value, **change} if route == PROFILES[0]['path'] else value
            with self.subTest(change=change), patch.object(self.api, 'request', side_effect=changed):
                with self.assertRaisesRegex(RuntimeError, 'unexpected profile'):
                    self.writer()

    def test_reject_each_immutable_field_change(self):
        self.writer()
        identifier = next(iter(self.api.rows))
        original = deepcopy(self.api.rows[identifier])
        for field in ('id', 'owner', 'name', 'profile', 'limits', 'created_at', 'color', 'nyxid_services'):
            self.api.rows[identifier] = deepcopy(original)
            self.api.rows[identifier][field] = 'corrupt'
            with self.subTest(field=field), self.assertRaises(RuntimeError):
                self.fixture.exercise('new-read-old-write', [])

    def test_reject_runtime_advancement_rewind_or_native_hierarchy(self):
        self.writer()
        identifier = next(iter(self.api.rows))
        original = deepcopy(self.api.rows[identifier])
        for extra in ({'revision': 0}, {'revision': True}, {'status': 'active'},
                      {'runtime_version': 'unexpected-image'}):
            self.api.rows[identifier] = {**original, **extra}
            with self.subTest(extra=extra), self.assertRaises(RuntimeError):
                self.fixture.exercise('new-read-old-write', [])
        self.api.rows[identifier] = original
        request = self.api.request
        def with_agent(method, route, **kwargs):
            value = request(method, route, **kwargs)
            if isinstance(value, dict) and 'agent_count' in value:
                value['agent_count'] = 1
            return value
        with patch.object(self.api, 'request', side_effect=with_agent), self.assertRaises(RuntimeError):
            self.fixture.exercise('new-read-old-write', [])

    def test_failed_provision_can_append_history_without_rewriting_admission(self):
        self.writer()
        for identifier, row in self.api.rows.items():
            row.update(status='failed', revision=2)
            self.api.events[identifier].append({'sandbox_id': identifier, 'seq': 2,
                'appended_at': '2026-01-01T00:00:01Z', 'kind': {'type': 'work_step_failed'}})
        self.fixture.exercise('new-read-old-write', [])

    def test_event_rewrite_duplicate_omission_and_gap_are_rejected(self):
        self.writer()
        identifier = next(iter(self.api.rows))
        original = deepcopy(self.api.events[identifier])
        cases = [[], [{**original[0], 'seq': 2}], [{**original[0], 'appended_at': 'changed'}],
                 [original[0], {**original[0], 'seq': 2}],
                 [{**original[0], 'sandbox_id': 'wrong'}]]
        for rows in cases:
            self.api.events[identifier] = rows
            with self.subTest(rows=rows), self.assertRaises(RuntimeError):
                self.fixture.exercise('new-read-old-write', [])

    def test_replay_change_and_false_conflict_mutation_fail(self):
        self.writer()
        command = next(iter(self.api.commands.values()))
        command['result']['name'] = 'changed'
        with self.assertRaisesRegex(RuntimeError, 'replay changed'):
            self.fixture.exercise('new-read-old-write', [])
        command['result']['name'] = command['body']['name']
        request = self.api.request
        def mutate(method, route, **kwargs):
            value = request(method, route, **kwargs)
            if kwargs.get('status') == 422:
                next(iter(self.api.rows.values()))['name'] = 'overwritten'
            return value
        with patch.object(self.api, 'request', side_effect=mutate), self.assertRaises(RuntimeError):
            self.fixture.exercise('new-read-old-write', [])

    def test_missing_inventory_and_repeated_cursor_fail(self):
        self.writer()
        request = self.api.request
        for page in ({'items': []}, {'items': [], 'next': 'loop'},
                     {'items': [next(iter(self.api.rows.values()))], 'next': 'loop'}):
            def broken(method, route, **kwargs):
                return page if route.startswith('/api/v1/sandboxes?') else request(method, route, **kwargs)
            with self.subTest(page=page), patch.object(self.api, 'request', side_effect=broken):
                with self.assertRaises(RuntimeError):
                    self.fixture.exercise('new-read-old-write', [])

    def test_wrong_stage_provenance_or_unavailable_route_has_no_success_receipt(self):
        with self.assertRaises(RuntimeError):
            self.fixture.exercise('wrong', [])
        with patch.object(self.api, 'request', side_effect=RuntimeError('HTTP 503')):
            with self.assertRaisesRegex(RuntimeError, 'HTTP 503'):
                self.writer()
        self.assertEqual(list(self.output.iterdir()), [])
        self.writer()
        path = self.output / 'old-write-sandbox-admission.private.json'
        record = json.loads(path.read_text())
        record['writer'] = 'new-write'
        path.write_text(json.dumps(record))
        with self.assertRaisesRegex(RuntimeError, 'provenance'):
            self.fixture.exercise('new-read-old-write', [])

    def test_boundary_removes_runtime_mutations_and_rejects_added_authority(self):
        config = {'CHRONO_AUTH_MODE': 'provider', **{name: '0' for name in (
            'CHRONO_NATIVE_PREPARED_TARGET', 'CHRONO_NATIVE_MACHINE_PREPARED_TARGET',
            'CHRONO_NATIVE_WARM_POOL_TARGET', 'CHRONO_NATIVE_MACHINE_WARM_POOL_TARGET')}}
        objects = [{'kind': 'ConfigMap', 'metadata': {'name': 'api-env'}, 'data': config},
                   {'kind': 'Role', 'metadata': {'name': 'cma-runtime-sandbox-reclaimer'},
                    'rules': [{'resources': ['sandboxes'], 'verbs': ['get', 'create', 'delete']}]},
                   {'kind': 'RoleBinding', 'metadata': {'name': 'binding'},
                    'roleRef': {'kind': 'Role', 'name': 'cma-runtime-sandbox-reclaimer'}}]
        objects += [{'kind': 'Deployment', 'metadata': {'name': name}}
                    for name in ('api', 'mongo', 'identity')]
        original = deepcopy(objects)
        result = manifests(objects)
        self.assertEqual(objects, original)
        self.assertEqual(result[1]['rules'][0]['verbs'], ['get', 'list'])
        cases = [objects + [{'kind': 'Deployment', 'metadata': {'name': 'controller'}}],
                 objects + [{'kind': 'ClusterRoleBinding', 'metadata': {'name': 'bypass'}}]]
        for resource in ('pods/exec', '*'):
            bad = deepcopy(objects)
            bad[1]['rules'][0]['resources'] = [resource]
            cases.append(bad)
        bad = deepcopy(objects)
        bad[0]['data']['CHRONO_NATIVE_PREPARED_TARGET'] = '1'
        cases.append(bad)
        for bad in cases:
            with self.subTest(bad=bad), self.assertRaises(RuntimeError):
                manifests(bad)


if __name__ == '__main__':
    unittest.main()
