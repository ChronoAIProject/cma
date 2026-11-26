#!/usr/bin/env python3
"""Read the durable Sandbox control ledger and embedded admission audit.

The application fixture admits lifecycle commands through the public API. This
collector has a separate find-only Mongo role so the compatibility assertion
cannot be satisfied by HTTP receipts alone.
"""
from copy import deepcopy
from datetime import datetime
import hashlib
import json
from pathlib import Path
import re

from migration_catalog import job as catalog_job, STAGES
from migration_sandbox_fixture import require

COLLECTIONS = ('sandbox_controls',)
SCHEMA = 'cma-migration-sandbox-command-documents-v1'
CAPABILITIES = {'view_details', 'view_history', 'view_artifacts', 'view_files',
                'interact', 'resolve_approval', 'operate_agent', 'manage_sandbox',
                'manage_bridge', 'manage_sharing'}


def infrastructure(objects, namespace, password):
    if not re.fullmatch(r'[A-Za-z0-9_-]{24,64}', password):
        raise ValueError('invalid local reader password encoding')
    selected = {(obj['kind'], obj['metadata']['name']): obj for obj in objects}
    init = selected[('Job', 'mongo-init')]['spec']['template']['spec']['containers'][0]
    init['env'].append({'name': 'MATRIX_SANDBOX_COMMAND_READ_PASSWORD', 'value': password})
    privileges = [{'resource': {'db': 'matrix', 'collection': name}, 'actions': ['find']}
                  for name in COLLECTIONS]
    script = ('const m=db.getSiblingDB("matrix"); m.createRole({role:"matrix_sandbox_command_reader",'
              'privileges:' + json.dumps(privileges, separators=(',', ':')) + ',roles:[]}); '
              'm.createUser({user:"matrix_sandbox_command_reader",'
              'pwd:process.env.MATRIX_SANDBOX_COMMAND_READ_PASSWORD,'
              'roles:[{role:"matrix_sandbox_command_reader",db:"matrix"}]})')
    selected[('ConfigMap', 'mongo-init')]['data']['init.sh'] += (
        '\nmongosh --host mongo:27017 --username matrix_admin --password "$MATRIX_ADMIN_PASSWORD" '
        '--authenticationDatabase admin --quiet --eval ' + "'" + script + "'\n")
    uri = (f'mongodb://matrix_sandbox_command_reader:{password}@mongo:27017/matrix?authSource=matrix'
           '&replicaSet=matrix&serverSelectionTimeoutMS=5000&connectTimeoutMS=5000&socketTimeoutMS=5000')
    objects.extend([
        {'apiVersion': 'v1', 'kind': 'ConfigMap',
         'metadata': {'name': 'mongo-sandbox-command-code', 'namespace': namespace},
         'data': {'read.cjs': Path(__file__).with_suffix('.cjs').read_text()}},
        {'apiVersion': 'v1', 'kind': 'ConfigMap',
         'metadata': {'name': 'mongo-sandbox-command-auth', 'namespace': namespace},
         'data': {'MATRIX_SANDBOX_COMMAND_READ_URI': uri}},
    ])
    return objects


def job(namespace, image, stage):
    result = deepcopy(catalog_job(namespace, image, stage))
    result['metadata']['name'] = 'sandbox-command-documents-' + stage
    pod = result['spec']['template']['spec']
    container = pod['containers'][0]
    container['name'] = 'sandbox-command-documents'
    container['envFrom'] = [{'configMapRef': {'name': 'mongo-sandbox-command-auth'}}]
    pod['volumes'][0]['configMap']['name'] = 'mongo-sandbox-command-code'
    return result


def _integer(value):
    require(isinstance(value, dict) and len(value) == 1
            and next(iter(value)) in ('$numberInt', '$numberLong'),
            'canonical BSON integer missing')
    text = next(iter(value.values()))
    require(isinstance(text, str) and re.fullmatch(r'0|[1-9][0-9]{0,18}', text),
            'invalid BSON integer')
    number = int(text)
    maximum = 2**31 - 1 if '$numberInt' in value else 2**63 - 1
    require(number <= maximum, 'BSON integer exceeds its signed representation')
    return number


def normalize(value):
    if isinstance(value, dict):
        if any(k.startswith('$number') for k in value):
            return _integer(value)
        return {k: normalize(v) for k, v in value.items()}
    if isinstance(value, list):
        return [normalize(v) for v in value]
    require(type(value) not in (int, float), 'noncanonical BSON number')
    return value


def _fields(value, fields, name):
    require(isinstance(value, dict) and set(value) == set(fields), name + ' fields differ')


def _unsigned(value, name, minimum=0):
    require(type(value) is int and minimum <= value <= 2**63 - 1,
            name + ' is not a bounded unsigned integer')


def _timestamp(value, name):
    require(isinstance(value, str) and re.fullmatch(
        r'[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}'
        r'(\.[0-9]+)?(Z|[+-][0-9]{2}:[0-9]{2})', value), name + ' is not RFC3339')
    try:
        parsed = datetime.fromisoformat(value.replace('Z', '+00:00'))
    except ValueError as error:
        raise RuntimeError('sandbox migration: invalid ' + name) from error
    require(parsed.tzinfo is not None, name + ' has no timezone')


def _digest(value):
    # These tuples follow SandboxControlEnvelope::scope and Control::fingerprint.
    # Build the actor in Rust's tagged-field order; raw BSON key order is not authority.
    data = json.dumps(value, separators=(',', ':'), ensure_ascii=False).encode()
    return hashlib.sha256(data).hexdigest()


def _envelope(value, expected, scope):
    from migration_browser_fixture import OWNER
    _fields(value, ('command_id', 'sandbox_id', 'sandbox_created_at', 'owner', 'actor',
                   'authorization', 'command', 'expected_revision', 'request_fingerprint'),
            'control envelope')
    require(all(value[key] == expected[key] for key in (
        'command_id', 'sandbox_id', 'command', 'expected_revision')),
        'control envelope differs from API receipt')
    _unsigned(value['expected_revision'], 'expected revision', 1)
    _timestamp(value['sandbox_created_at'], 'Sandbox creation time')
    # Both pinned readers' resource_request_context attributes ordinary
    # authenticated controls to Api, including browser-cookie requests. Preserve
    # the explicit null delegation in the actor and its serialized scope hash.
    actor = {'kind': 'api', 'subject': OWNER, 'delegation_id': None}
    require(value['owner'] == {'kind': 'person', 'subject': OWNER} and value['actor'] == actor,
            'control owner or actor differs')
    authority = value['authorization']
    _fields(authority, ('authority_id', 'policy_revision', 'grant_revision'), 'control authority')
    identifier = authority['authority_id']
    require(isinstance(identifier, str) and identifier == identifier.strip()
            and 0 < len(identifier.encode()) <= 256, 'invalid retained authority identity')
    _unsigned(authority['policy_revision'], 'authority policy revision', 1)
    if authority['grant_revision'] is not None:
        _unsigned(authority['grant_revision'], 'authority grant revision', 1)
    require(value['request_fingerprint'] == _digest(
        ['sandbox-control-v1', value['command'], value['expected_revision']]),
        'control request fingerprint differs')
    require(scope == _digest(['sandbox-control-scope-v1', identifier, actor,
                             value['sandbox_id'], value['command']]),
            'control idempotency scope differs')


def _audit(value, envelope):
    _fields(value, ('target', 'sandbox_id', 'sandbox_created_at', 'owner', 'actor',
                   'policy_revision', 'capability', 'captured_at', 'observations'),
            'control admission audit')
    require(all(value[key] == envelope[key] for key in (
        'owner', 'actor', 'sandbox_id', 'sandbox_created_at'))
        and value['target'] == {'kind': 'sandbox', 'sandbox_id': envelope['sandbox_id']}
        and value['capability'] == 'manage_sandbox', 'control audit attribution differs')
    _unsigned(value['policy_revision'], 'resource policy revision')
    _timestamp(value['captured_at'], 'audit capture time')
    observations = value['observations']
    # The synthetic personal owner takes the early owner branch in AccessDecision.
    # Other authorization shapes require their own fixture and cannot widen this cell.
    require(isinstance(observations, list) and len(observations) == 1,
            'personal-owner audit observations differ')
    observation = observations[0]
    _fields(observation, ('basis', 'capabilities'), 'authority observation')
    capabilities = observation['capabilities']
    require(observation['basis'] == {'kind': 'personal_owner'}
            and isinstance(capabilities, list) and all(isinstance(c, str) for c in capabilities)
            and len(set(capabilities)) == len(capabilities)
            and set(capabilities) <= CAPABILITIES and 'manage_sandbox' in capabilities,
            'personal-owner audit capabilities differ')


def validate(data, records):
    require(isinstance(data, dict) and set(data) == {'schema', 'database', 'collections'}
            and data['schema'] == SCHEMA and data['database'] == 'matrix',
            'wrong Sandbox command snapshot')
    collections = data['collections']
    require(isinstance(collections, dict) and set(collections) == set(COLLECTIONS),
            'wrong Sandbox command collection scope')
    require(isinstance(records, list) and len(records) in (1, 2)
            and all(isinstance(r, dict) and isinstance(r.get('commands'), list)
                    and len(r['commands']) == 2 for r in records),
            'API command receipts missing')
    require([r.get('writer') for r in records] == ['old-write', 'new-write'][:len(records)],
            'API command writer provenance differs')
    commands = [c for record in records for c in record['commands']]
    from migration_sandbox_control_fixture import receipt
    for command in commands:
        require(isinstance(command, dict) and isinstance(command.get('receipt'), dict)
                and isinstance(command.get('key'), str) and command['key'],
                'invalid saved command observation')
        expected = command['receipt']
        require('sandbox_id' in expected and 'expected_revision' in expected,
                'saved control receipt lacks identity')
        receipt(expected, expected['sandbox_id'], expected['expected_revision'], True)
    saved = {c['receipt']['command_id']: c for c in commands}
    require(len(saved) == 2 * len(records), 'API command receipts duplicated')
    controls = collections['sandbox_controls']
    require(isinstance(controls, list) and len(controls) == len(saved),
            'control row count differs from API receipts')
    seen = set()
    for row in controls:
        require(isinstance(row, dict) and set(row) == {'_id', 'document_schema_version',
            'sandbox_id', 'scope', 'key_sha256', 'queued', 'revision', 'queue_deadline', 'record'}
            and row['_id'] in saved and row['_id'] not in seen, 'control row identity differs')
        seen.add(row['_id'])
        command = saved[row['_id']]
        expected = command['receipt']
        require(_integer(row['document_schema_version']) == 1
                and _integer(row['revision']) == 2 and row['queued'] is False
                and row['sandbox_id'] == expected['sandbox_id']
                and row['queue_deadline'] == expected['queue_deadline']
                and isinstance(row['scope'], str) and re.fullmatch(r'[a-f0-9]{64}', row['scope'])
                and row['key_sha256'] == hashlib.sha256(command['key'].encode()).hexdigest(),
                'control schema, state, scope or key binding differs')
        value = normalize(row['record'])
        require(isinstance(value, dict) and set(value) == {'schema_version', 'envelope', 'key_sha256',
            'admitted_at', 'queue_deadline', 'status', 'revision', 'admission_audit', 'execution_audit'},
            'control aggregate shape differs')
        _unsigned(value['schema_version'], 'control schema version', 1)
        _unsigned(value['revision'], 'control record revision', 1)
        envelope = value['envelope']
        _envelope(envelope, expected, row['scope'])
        require(value['schema_version'] == 1 and value['revision'] == 2
                and value['key_sha256'] == row['key_sha256'] and value['execution_audit'] is None
                and value['status'] == expected['state'] and all(value[k] == expected[k]
                    for k in ('admitted_at', 'queue_deadline'))
                and all(envelope[k] == expected[k] for k in (
                    'command_id', 'sandbox_id', 'command', 'expected_revision')),
                'control aggregate differs from observed API receipt')
        _audit(value['admission_audit'], envelope)
    return collections


def compare(before, after, writer):
    for name in COLLECTIONS:
        old = {row['_id']: row for row in before[name]}
        new = {row['_id']: row for row in after[name]}
        require(len(old) == len(before[name]) and len(new) == len(after[name]),
                'duplicate durable ' + name)
        if not writer:
            require(new == old, 'retained reader changed durable ' + name)
        else:
            require(all(new.get(key) == row for key, row in old.items())
                    and len(new) >= len(old),
                    'new writer rewrote retained ' + name)


def capture(campaign, namespace, image, stage, source, application_image):
    require(stage in STAGES, 'unknown Sandbox command capture stage')
    manifest = job(namespace, image, stage)
    name = manifest['metadata']['name']
    campaign.kubectl(['apply', '-f', '-'], input_text=json.dumps(manifest))
    try:
        campaign.kubectl(['-n', namespace, 'wait', '--for=condition=complete', 'job/' + name,
                          '--timeout=125s'], timeout=135)
    except Exception:
        try:
            raw = campaign.kubectl(['-n', namespace, 'logs', 'job/' + name,
                                    '-c', 'sandbox-command-documents', '--tail=30'], timeout=20)
            (campaign.output / ('sandbox-command-documents-' + stage + '.failed.log')).write_text(raw)
        except Exception as error:
            (campaign.output / ('sandbox-command-documents-' + stage + '.diagnostic-error.json')).write_text(
                json.dumps({'type': type(error).__name__, 'message': 'failed Job log unavailable'}) + '\n')
        raise
    raw = campaign.kubectl(['-n', namespace, 'logs', 'job/' + name,
                            '-c', 'sandbox-command-documents'], timeout=20)
    path = campaign.output / ('sandbox-command-documents-' + stage + '.private.json')
    path.write_text(raw)
    data = json.loads(raw)
    writers = ('old-write',) if STAGES.index(stage) < 2 else ('old-write', 'new-write')
    records = [json.loads((campaign.output / (writer + '-sandbox-controls.private.json')).read_text())
               for writer in writers]
    rows = validate(data, records)
    index = STAGES.index(stage)
    if index:
        previous = json.loads((campaign.output / ('sandbox-command-documents-'
                                                  + STAGES[index - 1] + '.private.json')).read_text())
        compare(previous['collections'], rows, stage == 'new-write')
    return {'stage': stage, 'source': source, 'image': application_image,
            'path': path.name, 'sha256': hashlib.sha256(path.read_bytes()).hexdigest(),
            'collections': list(COLLECTIONS), 'commands': len(records) * 2,
            'outcome': 'PASS'}


if __name__ == '__main__':
    raise SystemExit('collector is launched as a bounded Mongo Job')
