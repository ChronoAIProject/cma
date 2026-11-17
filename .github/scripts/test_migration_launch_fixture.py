"""Reject loss, changed records and provider reconstruction in retained readers."""
from copy import deepcopy
import json
import secrets
from pathlib import Path
from types import SimpleNamespace
import tempfile
import unittest
from unittest.mock import Mock, patch

from migration_launch_fixture import HANDOFFS, LaunchFixture, LaunchProfileFixture, SERVICE


class MemoryAPI:
    def __init__(self, output):
        self.output = output
        self.provider = {'creates': 0, 'reads': 0, 'links': {}}
        self.launches, self.handoffs, self.commands, self.intents = {}, {}, {}, {}
        self.browser = None
        self.admin_bearer = None
        self.native = []
        self.settings = {'revision': 1, 'settings': {
            'triggers': {'enabled': False, 'orchestration_paused': False}, 'other': 'preserved'}}

    def admin_request(self, method, route, body=None):
        assert route == '/api/v1/admin/settings'
        if method == 'PATCH':
            assert body['expected_revision'] == self.settings['revision']
            self.settings['revision'] += 1
            self.settings['settings']['triggers'] = body['triggers']
        return deepcopy(self.settings)

    def control(self, method='GET', suffix='state'):
        if method == 'POST':
            key, state = suffix.split('/')
            self.provider['links'][key] = state
        return deepcopy(self.provider)

    def request(self, method, route, body=None, key=None, status=200):
        if method == 'POST' and key in self.commands:
            return deepcopy(self.commands[key])
        if method == 'GET':
            if route.startswith(('/api/v1/sandboxes?', '/api/v1/agents?')):
                return {'items': self.native, 'next': None}
            if route.startswith('/api/v1/service-connections/'):
                row = self.handoffs[route]
                if row['state'] == 'pending':
                    self.provider['reads'] += 1
                    observed = self.provider['links'][row['external']]
                    row['state'] = {'completed': 'satisfied', 'refused': 'failed'}.get(observed, observed)
                return deepcopy({k: v for k, v in row.items() if k != 'external'})
            for path, launch in self.launches.items():
                if route == path:
                    return deepcopy(launch)
                if route == path + '/requirements':
                    return deepcopy(launch['requirements'])
                if route.startswith(path + '/events?'):
                    return {'launch_id': launch['id'], 'next_cursor': launch['event_head'],
                            'events': [{'launch_id': launch['id'], 'seq': i, 'kind': 'test'}
                                       for i in range(1, launch['event_head'] + 1)]}
            raise AssertionError('unhandled GET ' + route)
        if route == '/api/v1/triggers':
            result = {'id': 'trigger'}
        elif route.endswith('/publish'):
            result = {'public_id': 'public'}
        elif route.endswith('/launches'):
            if not self.settings['settings']['triggers']['enabled']:
                raise RuntimeError('503 triggers_disabled')
            assert body == {'start_if_ready': False}
            identifier = 'launch' + str(len(self.launches))
            result = {'id': identifier, 'state': 'draft', 'event_head': 1,
                      'requirements': {'services': [{'service': SERVICE, 'state': 'missing'}]}}
            self.launches['/api/v1/launches/' + identifier] = deepcopy(result)
        elif route.endswith('/cancel'):
            result = self.launches[route.removesuffix('/cancel')]
            result.update(state='cancelled', event_head=3)
        elif route == '/api/v1/service-connections':
            if body['intent_id'] in self.intents:
                result = self.intents[body['intent_id']]
            else:
                external = 'link_' + str(len(self.handoffs))
                result = {'handoff_id': 'handoff' + str(len(self.handoffs)), 'hosted_url': 'https://fixture.test',
                          'expires_at': '2030-01-01T00:00:00Z'}
                self.provider['creates'] += 1
                self.provider['links'][external] = 'pending'
                self.handoffs[route + '/' + result['handoff_id']] = {
                    'handoff_id': result['handoff_id'], 'launch_id': body['launch_id'], 'service_id': SERVICE,
                    'expires_at': result['expires_at'], 'updated_at': '2026-11-17T00:00:00Z',
                    'state': 'pending', 'external': external}
                self.intents[body['intent_id']] = deepcopy(result)
        else:
            raise AssertionError('unhandled POST ' + route)
        self.commands[key] = deepcopy(result)
        return deepcopy(result)


class LaunchFixtureTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.api = MemoryAPI(Path(temp.name))
        self.fixture = LaunchFixture(self.api)
        self.fixture.control = self.api.control
        self.fixture.author.prepare_profiles = Mock()
        self.profiles = [{'kind': kind, 'created': {'id': kind}, 'revision': {'revision': 1}}
                         for kind in ('agent', 'workspace', 'sandbox')]

    def record(self):
        self.fixture.enable_launches('old-write')
        return self.fixture.write('old-write', self.profiles)

    def test_both_writer_reader_directions_preserve_all_variants(self):
        for stage in ('old-write', 'new-read-old-write', 'new-write', 'old-read-new-write'):
            receipt = self.fixture.exercise(stage, self.profiles)
            self.assertTrue(all(receipt['checks'].values()))
            self.assertEqual(receipt['samples'], 7)
            observations = json.loads((self.api.output / ('launch-verification-' + stage + '.private.json')).read_text())
            self.assertEqual(len(observations), 5)
            for row in observations:
                self.assertEqual(row['before']['creates'], row['after']['creates'])
                self.assertEqual(row['after']['reads'] - row['before']['reads'], row['state'] == 'pending')
        self.assertEqual(self.api.provider['creates'], 10)

    def test_lost_or_changed_launch_data_is_rejected(self):
        record = self.record()
        route = record['launches'][0]['route']
        for field, value in [('state', 'resolving'), ('started_at', 'now'), ('event_head', 201),
                             ('requirements', {'services': []}), ('id', 'different')]:
            original = deepcopy(self.api.launches[route])
            self.api.launches[route][field] = value
            with self.subTest(field=field), self.assertRaises(RuntimeError):
                self.fixture.verify(record)
            self.api.launches[route] = original

    def test_incomplete_reordered_or_foreign_events_are_rejected(self):
        record = self.record()
        original = self.fixture.request
        for fault in ('missing', 'order', 'foreign'):
            def altered(method, route, **kwargs):
                value = original(method, route, **kwargs)
                if '/events?' in route:
                    if fault == 'missing': value['events'] = []
                    elif fault == 'order': value['events'][0]['seq'] = 2
                    else: value['events'][0]['launch_id'] = 'foreign'
                return value
            self.fixture.request = altered
            with self.subTest(fault=fault), self.assertRaises(RuntimeError):
                self.fixture.verify(record)
        self.fixture.request = original

    def test_different_handoff_or_repeated_provider_effect_fails(self):
        record = self.record()
        original = self.fixture.request
        for fault in ('changed', 'create', 'read', 'replay'):
            def altered(method, route, **kwargs):
                value = original(method, route, **kwargs)
                if '/service-connections' in route:
                    if fault == 'changed' and method == 'GET': value['service_id'] = 'other'
                    if fault == 'create': self.api.provider['creates'] += 1
                    if fault == 'read': self.api.provider['reads'] += 1
                    if fault == 'replay' and method == 'POST': value['handoff_id'] = 'duplicate'
                return value
            self.fixture.request = altered
            with self.subTest(fault=fault), self.assertRaises(RuntimeError):
                self.fixture.verify(record)
        self.fixture.request = original

    def test_launch_replay_side_effect_or_changed_response_is_rejected(self):
        record = self.record()
        original = self.fixture.request
        for fault in ('response', 'effect'):
            def altered(method, route, **kwargs):
                value = original(method, route, **kwargs)
                if method == 'POST' and route.endswith('/launches'):
                    if fault == 'response': value['id'] = 'duplicate'
                    else: self.api.launches[record['launches'][0]['route']]['event_head'] += 1
                return value
            self.fixture.request = altered
            with self.subTest(fault=fault), self.assertRaises(RuntimeError):
                self.fixture.verify(record)

    def test_missing_variants_wrong_provenance_and_native_creation_fail(self):
        record = self.record()
        for field in ('launches', 'handoffs'):
            bad = deepcopy(record)
            bad[field].pop()
            with self.assertRaisesRegex(RuntimeError, 'variants missing'):
                self.fixture.verify(bad)
        bad = deepcopy(record)
        bad['handoffs'][0]['state'] = 'unknown'
        with self.assertRaisesRegex(RuntimeError, 'variants differ'):
            self.fixture.verify(bad)
        with self.assertRaisesRegex(RuntimeError, 'unsupported stage'):
            self.fixture.exercise('unknown', self.profiles)
        record['writer'] = 'new-write'
        self.api.settings['settings']['triggers']['enabled'] = True
        (self.api.output / 'old-write-launches.private.json').write_text(json.dumps(record))
        with self.assertRaisesRegex(RuntimeError, 'provenance'):
            self.fixture.exercise('new-read-old-write', self.profiles)
        self.api.native = [{'id': 'unexpected'}]
        with self.assertRaisesRegex(RuntimeError, 'allocated native'):
            self.fixture.no_runtime()

    def test_disabled_default_is_enabled_once_and_other_settings_preserved(self):
        before = deepcopy(self.api.settings)
        self.fixture.enable_launches('old-write')
        self.assertTrue(self.api.settings['settings']['triggers']['enabled'])
        self.assertEqual(self.api.settings['revision'], before['revision'] + 1)
        self.assertEqual(self.api.settings['settings']['other'], before['settings']['other'])
        self.assertEqual(self.api.settings['settings']['triggers']['orchestration_paused'],
                         before['settings']['triggers']['orchestration_paused'])
        after = deepcopy(self.api.settings)
        self.fixture.enable_launches('new-read-old-write')
        self.assertEqual(self.api.settings, after)
        self.assertTrue((self.api.output / 'local-trigger-enablement.private.json').exists())

    def test_reader_does_not_repair_lost_enablement(self):
        for stage in ('new-read-old-write', 'new-write', 'old-read-new-write'):
            with self.subTest(stage=stage), self.assertRaisesRegex(RuntimeError, 'enablement disappeared'):
                self.fixture.enable_launches(stage)
        self.assertFalse(self.api.settings['settings']['triggers']['enabled'])

    def test_settings_rejection_or_nonpersisted_write_fails(self):
        original = self.api.admin_request
        def ignored(method, route, body=None):
            if method == 'PATCH': return deepcopy(self.api.settings)
            return original(method, route, body)
        self.api.admin_request = ignored
        with self.assertRaisesRegex(RuntimeError, 'persist exactly'):
            self.fixture.enable_launches('old-write')
        self.api.admin_request = Mock(side_effect=RuntimeError('settings conflict'))
        with self.assertRaisesRegex(RuntimeError, 'settings conflict'):
            self.fixture.enable_launches('old-write')
        self.api.admin_request = Mock(return_value={'revision': 1, 'settings': {}})
        with self.assertRaisesRegex(RuntimeError, 'policy missing'):
            self.fixture.enable_launches('old-write')

    def test_profile_adapter_adds_only_the_missing_service(self):
        adapter = object.__new__(LaunchProfileFixture)
        body = {'content': {'nyxid_services': []}, 'name': 'same'}
        with patch('migration_profile_fixture.Fixture.create', return_value='result') as create:
            self.assertEqual(adapter.create('sandbox', body), 'result')
            self.assertEqual(create.call_args.args, ('sandbox', {'name': 'same', 'content': {'nyxid_services': [SERVICE]}}))
            adapter.create('agent', body)
            self.assertIs(create.call_args.args[1], body)
        self.assertEqual(body['content']['nyxid_services'], [])

    def test_control_checks_response_and_does_not_log_credentials(self):
        fixture = LaunchFixture(self.api)
        with self.assertRaisesRegex(RuntimeError, 'control missing'):
            fixture.control()
        self.api.admin_bearer = secrets.token_urlsafe(32)
        self.api.browser = SimpleNamespace(identity='http://127.0.0.1:1234', raw=Mock())
        for code, body in [(403, {}), (200, {'creates': -1, 'reads': 0, 'links': {}})]:
            self.api.browser.raw.return_value = (code, {}, json.dumps(body).encode())
            with self.assertRaises(RuntimeError): fixture.control()
        self.api.browser.raw.return_value = (200, {}, json.dumps(self.api.provider).encode())
        self.assertEqual(fixture.control(), self.api.provider)


if __name__ == '__main__':
    unittest.main()
