#!/usr/bin/env python3
"""Read actual gateway record writes across both immutable AM image switches.

The two collections deliberately share UUIDs and have different opaque bytes.
This makes collection aliasing visible without involving a native runtime or
pretending that the opaque blobs are valid external gateway credentials.
"""
import base64
import json
from pathlib import Path
from urllib.parse import urlencode
import uuid

from migration_profile_fixture import main

KINDS = ('channel', 'connection')
COLLECTIONS = ['gateway_' + kind + '_records' for kind in KINDS]
SHAPES = ['gateway-binary-record', 'gateway-owner-transfer', 'gateway-version-fence',
          'gateway-namespace-separation', 'gateway-record-deletion', 'gateway-paged-heads']
STAGES = ('old-write', 'new-read-old-write', 'new-write', 'old-read-new-write')


def require(condition, message):
    if not condition:
        raise RuntimeError('gateway migration: ' + message)


class GatewayFixture:
    def __init__(self, fixture):
        self.fixture = fixture
        self.output = Path(fixture.output)

    def request(self, method, path, body=None, status=200):
        return self.fixture.admin_request(method, path, body=body, status=status)

    @staticmethod
    def path(kind, identifier=''):
        return '/api/v1/gateway/' + kind + '-records' + ('/' + identifier if identifier else '')

    def put(self, path, owner, blob, expected=None, status=200):
        return self.request('PUT', path, {'owner': owner, 'blob': blob,
                                         'expected_version': expected}, status)

    def view(self, path, owner, blob, version):
        row = self.request('GET', path)
        require(isinstance(row, dict) and set(row) == {
            'record_id', 'owner', 'blob', 'version', 'created_at', 'updated_at'},
            'record response shape differs')
        require(row['record_id'] == path.rsplit('/', 1)[-1]
                and row['owner'] == owner and row['blob'] == blob
                and type(row['version']) is int and row['version'] == version,
                'record identity, owner, bytes or version differs')
        require(all(isinstance(row[k], str) and row[k] for k in ('created_at', 'updated_at')),
                'record timestamps missing')
        return row

    def heads(self, kind, owner):
        rows, cursor, seen = [], None, set()
        for _ in range(10):
            query = {'owner': owner, 'limit': 1}
            if cursor:
                query['after'] = cursor
            page = self.request('GET', self.path(kind) + '?' + urlencode(query))
            require(isinstance(page, dict) and set(page) == {'items', 'next_after'}
                    and isinstance(page['items'], list) and len(page['items']) <= 1,
                    'invalid record page')
            for row in page['items']:
                require(isinstance(row, dict) and set(row) == {
                    'record_id', 'owner', 'version', 'updated_at'}, 'invalid record head')
                require(row['owner'] == owner and row['record_id'] not in seen,
                        'record page repeats or leaks another owner')
                if rows:
                    require(rows[-1]['record_id'] < row['record_id'], 'unordered record page')
                rows.append(row)
                seen.add(row['record_id'])
            after = page['next_after']
            if after is None:
                return rows
            require(page['items'] and after == page['items'][-1]['record_id'] and after != cursor,
                    'record cursor made no progress')
            cursor = after
        raise RuntimeError('gateway migration: record page bound exceeded')

    def verify(self, record):
        for kind in KINDS:
            saved = record['kinds'][kind]
            for row in saved['rows']:
                path = self.path(kind, row['record_id'])
                actual = self.view(path, row['owner'], row['blob'], row['version'])
                require(actual == row, 'reader changed a retained record')
                # Gateway PUT is CAS, not idempotency-key replay: repeating a
                # create or stale update must conflict without changing bytes.
                for expected in (None, 0):
                    error = self.put(path, 'wrong-owner', row['blob'], expected, 409)
                    require(error.get('code') == 'conflict', 'wrong stale-write problem')
                error = self.request('DELETE', path + '?expected_version=0', status=409)
                require(error.get('code') == 'conflict', 'wrong stale-delete problem')
                require(self.request('GET', path) == row, 'rejected mutation changed state')
                denied = self.fixture.request('GET', path, status=403)
                require(denied.get('code') == 'forbidden', 'ordinary owner gained gateway authority')
            for owner in (record['owner'], record['moved_owner']):
                expected = sorted(({key: row[key] for key in (
                    'record_id', 'owner', 'version', 'updated_at')}
                    for row in saved['rows'] if row['owner'] == owner),
                    key=lambda row: row['record_id'])
                require(self.heads(kind, owner) == expected, 'paged heads differ from records')
            missing = self.request('GET', self.path(kind, record['deleted_id']), status=404)
            require(missing.get('code') == 'gateway_record_not_found', 'deleted record revived')

    def write(self, stage):
        record = {'writer': stage, 'owner': 'matrix-gateway-' + stage,
                  'moved_owner': 'matrix-gateway-moved-' + stage,
                  'deleted_id': str(uuid.uuid4()), 'kinds': {}}
        identifiers = [str(uuid.uuid4()) for _ in range(3)]
        for index, kind in enumerate(KINDS):
            rows = []
            payloads = [bytes([index]), bytes(range(256)) * 256,
                        b'opaque\x00\xff' + kind.encode()]
            for offset, (identifier, payload) in enumerate(zip(identifiers, payloads)):
                # Both namespaces have identical identifiers but different data.
                if offset == 1 and index:
                    payload = payload[::-1]
                blob = base64.b64encode(payload).decode()
                path = self.path(kind, identifier)
                owner = record['owner']
                created = self.put(path, owner, blob)
                require(created.get('record_id') == identifier and created.get('version') == 1,
                        'create did not return version 1')
                row = self.view(path, owner, blob, 1)
                require(created == {key: row[key] for key in ('record_id', 'version', 'updated_at')},
                        'write receipt differs from actual record')
                if offset == 2:
                    owner = record['moved_owner']
                    changed = self.put(path, owner, blob, 1)
                    updated = self.view(path, owner, blob, 2)
                    require(updated['created_at'] == row['created_at']
                            and changed == {key: updated[key] for key in (
                                'record_id', 'version', 'updated_at')}, 'owner transfer changed history')
                    row = updated
                rows.append(row)
            deleted_path = self.path(kind, record['deleted_id'])
            self.put(deleted_path, record['owner'], base64.b64encode(b'delete').decode())
            require(self.request('DELETE', deleted_path + '?expected_version=1', status=204) is None,
                    'successful deletion unexpectedly returned a body')
            record['kinds'][kind] = {'rows': rows}
        self.verify(record)
        return record

    def exercise(self, stage, profiles):
        require(stage in STAGES, 'unsupported stage')
        writer = stage if stage in ('old-write', 'new-write') else (
            'old-write' if stage == 'new-read-old-write' else 'new-write')
        path = self.output / (writer + '-gateway.private.json')
        if stage == writer:
            record = self.write(stage)
            path.write_text(json.dumps(record, indent=2) + '\n')
        else:
            record = json.loads(path.read_text())
            require(record.get('writer') == writer and set(record.get('kinds', {})) == set(KINDS),
                    'writer provenance or collection set differs')
            self.verify(record)
        return {'collections': COLLECTIONS, 'shapes': SHAPES, 'samples': 6,
                'checks': {name: True for name in SHAPES}}


if __name__ == '__main__':
    main(GatewayFixture)
