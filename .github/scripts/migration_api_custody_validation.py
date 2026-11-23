"""Join encrypted BSON custody to exact API credentials and command authority."""
import base64
import binascii
import hashlib
import json
import re

from migration_api_custody_provider import ISSUER, SLOTS
from migration_browser_fixture import OWNER
from migration_sandbox_documents import _integer, validate as validate_commands
from migration_sandbox_fixture import require

SCHEMA = 'cma-migration-api-custody-documents-v1'
COLLECTION = 'auth_api_bearers'


def binary(value, length=None):
    require(isinstance(value, dict) and set(value) == {'$binary'}, 'custody BSON binary missing')
    encoded = value['$binary']
    require(isinstance(encoded, dict) and set(encoded) == {'base64', 'subType'}
            and encoded['subType'] == '00' and isinstance(encoded['base64'], str),
            'custody BSON binary encoding differs')
    try:
        raw = base64.b64decode(encoded['base64'], validate=True)
    except (ValueError, binascii.Error) as error:
        raise RuntimeError('custody BSON base64 invalid') from error
    require(base64.b64encode(raw).decode() == encoded['base64']
            and (length is None or len(raw) == length), 'custody BSON binary length or canonical form differs')
    return raw


def validate(data, records, command_data):
    validate_commands(command_data, records)
    require(isinstance(data, dict) and set(data) == {'schema', 'database', 'collections'}
            and data['schema'] == SCHEMA and data['database'] == 'matrix'
            and isinstance(data['collections'], dict)
            and set(data['collections']) == {COLLECTION}, 'wrong custody snapshot')
    expected, authorities = {}, {}
    for record in records:
        for index, command in enumerate(record['commands']):
            custody = command.get('custody')
            require(isinstance(custody, dict) and set(custody) == {'slot', 'issuer', 'subject',
                'credential_id', 'custody_id', 'expires_at', 'token_bytes'}, 'custody observation missing')
            require(custody['slot'] == f"{record['writer']}/{index}" and custody['slot'] in SLOTS
                    and custody['issuer'] == ISSUER and custody['subject'] == OWNER,
                    'custody writer or principal differs')
            for key in ('credential_id', 'custody_id'):
                require(isinstance(custody[key], str) and re.fullmatch('[a-f0-9]{64}', custody[key]),
                        'custody digest invalid')
            require(type(custody['expires_at']) is int and 0 < custody['expires_at'] <= (2**63 - 1)//1000
                    and type(custody['token_bytes']) is int and 0 < custody['token_bytes'] <= 4096,
                    'custody expiry or token length invalid')
            identity = json.dumps(['cma-api-bearer-custody-v1', ISSUER, OWNER, custody['credential_id']],
                                  separators=(',', ':')).encode()
            key = hashlib.sha256(identity).hexdigest()
            require(key == custody['custody_id'] and key not in expected, 'custody identity mismatch or duplicate')
            expected[key] = custody
            authorities[command['receipt']['command_id']] = f"api:{key}:{custody['expires_at']}"
    for row in command_data['collections']['sandbox_controls']:
        auth = row['record']['envelope']['authorization']
        require(auth['authority_id'] == authorities[row['_id']]
                and _integer(auth['policy_revision']) == 1 and auth['grant_revision'] is None,
                'control authority not bound to independent custody')
    rows = data['collections'][COLLECTION]
    require(isinstance(rows, list) and len(rows) == len(expected), 'custody row count differs')
    seen = set()
    for row in rows:
        require(isinstance(row, dict) and set(row) == {'_id', 'document_schema_version', 'issuer',
                'subject', 'credential_id', 'access_token', 'expires_at'}, 'custody document fields differ')
        key = binary(row['_id'], 32).hex()
        require(key in expected and key not in seen, 'foreign or duplicate custody document')
        seen.add(key)
        meta = expected[key]
        require(_integer(row['document_schema_version']) == 1
                and all(row[k] == meta[k] for k in ('issuer', 'subject', 'credential_id')),
                'custody schema or identity fields differ')
        require(row['expires_at'] == {'$date': {'$numberLong': str(meta['expires_at'] * 1000)}},
                'custody expiry was extended or changed')
        envelope = row['access_token']
        require(isinstance(envelope, dict) and set(envelope) == {'version', 'nonce', 'ciphertext'}
                and _integer(envelope['version']) == 1, 'custody cipher envelope differs')
        binary(envelope['nonce'], 12)
        binary(envelope['ciphertext'], meta['token_bytes'] + 16)
    return rows


def compare(before, after, writer):
    def keyed(rows):
        result = {binary(row['_id'], 32).hex(): row for row in rows}
        require(len(result) == len(rows), 'duplicate custody snapshot identity')
        return result
    old, new = keyed(before), keyed(after)
    require(all(new.get(key) == row for key, row in old.items()), 'retained encrypted custody changed')
    require(len(new) == len(old) + (2 if writer else 0), 'unexpected custody growth at reader switch')
