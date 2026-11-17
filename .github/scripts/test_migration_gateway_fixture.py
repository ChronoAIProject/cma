#!/usr/bin/env python3
"""Fault-injection contracts for the gateway reader/writer assertions."""
import base64
from copy import deepcopy
import importlib.util
import json
from pathlib import Path
import secrets
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
from urllib.parse import parse_qs, urlsplit

from migration_gateway_fixture import GatewayFixture, STAGES
from migration_gateway_provider import configure, manifests
from migration_kind_manifests import manifests as base_manifests
from migration_profile_fixture import Fixture


class RecordAPI:
    """Minimal HTTP contract double; application compatibility needs kind evidence."""
    def __init__(self, output):
        self.output, self.rows, self.calls = Path(output), {}, []

    def request(self, method, path, body=None, status=200):
        if status != 403:
            raise AssertionError('ordinary user must be denied')
        return {'code': 'forbidden'}

    def admin_request(self, method, path, body=None, status=200):
        self.calls.append((method, path, deepcopy(body), status))
        url = urlsplit(path)
        query = parse_qs(url.query)
        parts = url.path.split('/')
        kind = parts[4]
        identifier = parts[5] if len(parts) > 5 else None
        key = (kind, identifier)
        row = self.rows.get(key)
        code, result = 200, None
        if method == 'GET' and identifier is None:
            rows = sorted((row for (ns, _), row in self.rows.items()
                           if ns == kind and row['owner'] == query['owner'][0]
                           and row['record_id'] > query.get('after', [''])[0]),
                          key=lambda row: row['record_id'])[:int(query['limit'][0])]
            result = {'items': [{k: row[k] for k in (
                'record_id', 'owner', 'version', 'updated_at')} for row in rows],
                'next_after': rows[-1]['record_id'] if rows else None}
        elif method == 'GET':
            code, result = (200, row) if row else (404, {'code': 'gateway_record_not_found'})
        elif method == 'PUT':
            expected = body['expected_version']
            if (row is None and expected is None) or (row is not None and expected == row['version']):
                version = row['version'] + 1 if row else 1
                row = {'record_id': identifier, 'owner': body['owner'], 'blob': body['blob'],
                       'version': version, 'created_at': row['created_at'] if row else 'created',
                       'updated_at': 'updated-' + str(version)}
                self.rows[key] = row
                result = {k: row[k] for k in ('record_id', 'version', 'updated_at')}
            else:
                code, result = 409, {'code': 'conflict'}
        elif method == 'DELETE':
            if row and int(query['expected_version'][0]) == row['version']:
                del self.rows[key]
                code = 204
            else:
                code, result = 409, {'code': 'conflict'}
        else:
            raise AssertionError('unexpected method')
        if code != status:
            raise RuntimeError(f'HTTP {code}, expected {status}')
        return deepcopy(result)


class GatewayFixtureTests(unittest.TestCase):
    def test_unmounted_route_fails_before_emitting_a_compatibility_receipt(self):
        with tempfile.TemporaryDirectory() as directory:
            api = RecordAPI(directory)
            with patch.object(api, 'admin_request', side_effect=RuntimeError('HTTP 404')) as request:
                with self.assertRaisesRegex(RuntimeError, 'HTTP 404'):
                    GatewayFixture(api).exercise('old-write', [])
            self.assertEqual(request.call_count, 1)
            self.assertEqual(list(Path(directory).iterdir()), [])

    def test_all_four_stages_preserve_records_and_exact_binary_boundaries(self):
        with tempfile.TemporaryDirectory() as directory:
            api = RecordAPI(directory)
            fixture = GatewayFixture(api)
            for stage in STAGES:
                result = fixture.exercise(stage, [])
                self.assertEqual(result['samples'], 6)
                self.assertEqual(len(result['collections']), 2)
                self.assertTrue(all(result['checks'].values()))
            self.assertEqual(len(api.rows), 12)
            sizes = {len(base64.b64decode(row['blob'])) for row in api.rows.values()}
            self.assertTrue({1, 65536}.issubset(sizes))
            records = json.loads((Path(directory) / 'new-write-gateway.private.json').read_text())
            channel = records['kinds']['channel']['rows']
            connection = records['kinds']['connection']['rows']
            for left, right in zip(channel, connection):
                self.assertEqual(left['record_id'], right['record_id'])
                self.assertNotEqual(left['blob'], right['blob'])

    def test_reader_rejects_each_retained_field_corruption(self):
        with tempfile.TemporaryDirectory() as directory:
            api = RecordAPI(directory)
            fixture = GatewayFixture(api)
            fixture.exercise('old-write', [])
            original = deepcopy(api.rows)
            key = next(iter(original))
            for field in ('record_id', 'owner', 'blob', 'version', 'created_at', 'updated_at'):
                with self.subTest(field=field):
                    api.rows = deepcopy(original)
                    api.rows[key][field] = 99 if field == 'version' else 'corrupt'
                    with self.assertRaises(RuntimeError):
                        fixture.exercise('new-read-old-write', [])

    def test_reader_rejects_namespace_alias_and_deleted_record_resurrection(self):
        with tempfile.TemporaryDirectory() as directory:
            api = RecordAPI(directory)
            fixture = GatewayFixture(api)
            fixture.exercise('old-write', [])
            original = deepcopy(api.rows)
            for (kind, identifier), row in list(api.rows.items()):
                if kind == 'connection-records':
                    api.rows[(kind, identifier)] = deepcopy(api.rows[('channel-records', identifier)])
            with self.assertRaises(RuntimeError):
                fixture.exercise('new-read-old-write', [])
            api.rows = original
            saved = json.loads((Path(directory) / 'old-write-gateway.private.json').read_text())
            row = deepcopy(next(iter(original.values())))
            row['record_id'] = saved['deleted_id']
            api.rows[('channel-records', saved['deleted_id'])] = row
            with self.assertRaises(RuntimeError):
                fixture.exercise('new-read-old-write', [])

    def test_false_conflict_that_mutates_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            api = RecordAPI(directory)
            fixture = GatewayFixture(api)
            fixture.exercise('old-write', [])
            request = api.admin_request
            def corrupt(method, path, body=None, status=200):
                response = request(method, path, body, status)
                if method == 'PUT' and status == 409:
                    next(iter(api.rows.values()))['owner'] = 'silently-overwritten'
                return response
            api.admin_request = corrupt
            with self.assertRaisesRegex(RuntimeError, 'rejected mutation changed state'):
                fixture.exercise('new-read-old-write', [])

    def test_page_omission_and_bad_cursors_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            api = RecordAPI(directory)
            fixture = GatewayFixture(api)
            fixture.exercise('old-write', [])
            request = api.admin_request
            for bad in ({'items': [], 'next_after': None}, {'items': [], 'next_after': 'loop'},
                        {'items': [], 'next': None}):
                def missing(method, path, body=None, status=200):
                    return bad if 'limit=' in path else request(method, path, body, status)
                with self.subTest(bad=bad), patch.object(api, 'admin_request', side_effect=missing):
                    with self.assertRaises(RuntimeError):
                        fixture.exercise('new-read-old-write', [])

    def test_wrong_writer_or_missing_namespace_is_not_accepted(self):
        with tempfile.TemporaryDirectory() as directory:
            fixture = GatewayFixture(RecordAPI(directory))
            fixture.exercise('old-write', [])
            path = Path(directory) / 'old-write-gateway.private.json'
            original = json.loads(path.read_text())
            for field, value in (('writer', 'new-write'), ('kinds', {})):
                path.write_text(json.dumps({**original, field: value}))
                with self.assertRaisesRegex(RuntimeError, 'provenance or collection set'):
                    fixture.exercise('new-read-old-write', [])
            with self.assertRaisesRegex(RuntimeError, 'unsupported stage'):
                fixture.exercise('other', [])


class GatewayDependencyTests(unittest.TestCase):
    def test_scope_extension_grants_only_the_admin(self):
        source = Path(__file__).with_name('multi-am-features') / 'owner' / 'identity.py'
        spec = importlib.util.spec_from_file_location('gateway_test_identity', source)
        identity = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(identity)
        original = list(identity.PERMISSIONS)
        provider = configure(identity)
        provider.config = {key: secrets.token_urlsafe(32) for key in (
            'bearer', 'admin_bearer', 'outsider_bearer')}
        actor = provider.__new__(provider)
        for key in ('bearer', 'outsider_bearer'):
            self.assertEqual(actor.actor(provider.config[key])[1], original)
        self.assertIn('cma:gateway:records', actor.actor(provider.config['admin_bearer'])[1])
        self.assertIsNone(actor.actor('unrecognized'))

    def test_manifest_adapter_is_opt_in_and_refuses_other_provider_overrides(self):
        credentials = {key: secrets.token_urlsafe(32) for key in (
            'client_id', 'client_secret', 'bearer', 'admin_bearer', 'outsider_bearer',
            'refresh_token', 'expires_at')}
        objects = base_manifests('image', 'runtime', 'admin', 'key', 'mongo', {},
                                 identity_credentials=credentials)
        saved = deepcopy(objects)
        changed = manifests(objects)
        self.assertEqual(objects, saved)
        self.assertNotEqual(changed, saved)
        with self.assertRaisesRegex(ValueError, 'already customized'):
            manifests(changed)
        trusted = base_manifests('image', 'runtime', 'admin', 'key', 'mongo', {})
        with self.assertRaisesRegex(ValueError, 'provider auth'):
            manifests(trusted)

    def test_empty_204_is_supported_and_other_empty_responses_stay_errors(self):
        fixture = Fixture.__new__(Fixture)
        fixture.base, fixture.cookies, fixture.requests = 'http://127.0.0.1:1', {}, []
        with patch('migration_profile_fixture.urlopen') as request:
            response = request.return_value.__enter__.return_value
            response.status, response.headers = 204, {}
            response.read.return_value = b''
            self.assertIsNone(fixture.request('DELETE', '/record', status=204))
            response.read.return_value = b'{}'
            with self.assertRaisesRegex(RuntimeError, '204 response'):
                fixture.request('DELETE', '/record', status=204)
            response.status, response.read.return_value = 200, b''
            with self.assertRaises(json.JSONDecodeError):
                fixture.request('GET', '/record')


if __name__ == '__main__':
    unittest.main()
