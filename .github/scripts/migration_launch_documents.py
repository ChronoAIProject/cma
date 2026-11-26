"""Read only three synthetic collections; join raw documents to actual API reads."""
from copy import deepcopy
import hashlib
import json
from pathlib import Path
import re

from migration_catalog import job as catalog_job, STAGES
from migration_launch_fixture import require
from migration_browser_fixture import OWNER

COLLECTIONS = ('trigger_launches', 'trigger_launch_input_protocols', 'visitor_launch_days')
SCHEMA = 'cma-migration-launch-documents-v1'


def infrastructure(objects, namespace, password):
    if not re.fullmatch(r'[A-Za-z0-9_-]{24,64}', password):
        raise ValueError('invalid local reader password encoding')
    selected = {(obj['kind'], obj['metadata']['name']): obj for obj in objects}
    init = selected[('Job', 'mongo-init')]['spec']['template']['spec']['containers'][0]
    init['env'].append({'name': 'MATRIX_LAUNCH_READ_PASSWORD', 'value': password})
    # Independent role: the existing catalog collector keeps metadata-only access.
    privileges = [{'resource': {'db': 'matrix', 'collection': name}, 'actions': ['find']}
                  for name in COLLECTIONS]
    script = ('const m=db.getSiblingDB("matrix"); m.createRole({role:"matrix_launch_reader",'
              'privileges:' + json.dumps(privileges, separators=(',', ':')) + ',roles:[]}); '
              'm.createUser({user:"matrix_launch_reader",pwd:process.env.MATRIX_LAUNCH_READ_PASSWORD,'
              'roles:[{role:"matrix_launch_reader",db:"matrix"}]})')
    selected[('ConfigMap', 'mongo-init')]['data']['init.sh'] += (
        '\nmongosh --host mongo:27017 --username matrix_admin --password "$MATRIX_ADMIN_PASSWORD" '
        '--authenticationDatabase admin --quiet --eval ' + "'" + script + "'\n")
    uri = (f'mongodb://matrix_launch_reader:{password}@mongo:27017/matrix?authSource=matrix'
           '&replicaSet=matrix&serverSelectionTimeoutMS=5000&connectTimeoutMS=5000&socketTimeoutMS=5000')
    for name, data in [('mongo-launch-code', {'read.cjs': Path(__file__).with_suffix('.cjs').read_text()}),
                       ('mongo-launch-auth', {'MATRIX_LAUNCH_READ_URI': uri})]:
        objects.append({'apiVersion': 'v1', 'kind': 'ConfigMap',
                        'metadata': {'name': name, 'namespace': namespace}, 'data': data})
    return objects


def job(namespace, image, stage):
    result = deepcopy(catalog_job(namespace, image, stage))
    result['metadata']['name'] = 'launch-documents-' + stage
    pod = result['spec']['template']['spec']
    container = pod['containers'][0]
    container['name'] = 'launch-documents'
    container['envFrom'] = [{'configMapRef': {'name': 'mongo-launch-auth'}}]
    pod['volumes'][0]['configMap']['name'] = 'mongo-launch-code'
    return result


def integer(value):
    require(isinstance(value, dict) and len(value) == 1
            and next(iter(value)) in ('$numberInt', '$numberLong'), 'canonical BSON integer missing')
    text = next(iter(value.values()))
    require(isinstance(text, str) and re.fullmatch(r'0|[1-9][0-9]{0,18}', text), 'invalid BSON integer')
    return int(text)


def validate(data, records, expected_count):
    require(isinstance(data, dict) and set(data) == {'schema', 'database', 'collections'}
            and data['schema'] == SCHEMA and data['database'] == 'matrix', 'wrong document snapshot')
    collections = data['collections']
    require(isinstance(collections, dict) and set(collections) == set(COLLECTIONS), 'wrong collection scope')
    require(expected_count in (2, 4) and len(records) == expected_count // 2, 'wrong writer sample count')
    days = {r['day'] for r in records}
    require(len(days) == 1, 'quota documents cross UTC days')
    day = days.pop()
    saved = {row['snapshot']['detail']['id']: row for record in records for row in record['launches']}
    require(len(saved) == expected_count, 'duplicate retained Launch')
    for name in COLLECTIONS:
        rows = collections[name]
        require(isinstance(rows, list) and len(rows) == (1 if name == 'visitor_launch_days' else expected_count),
                'nonempty document count differs for ' + name)
        require(all(isinstance(row, dict) and isinstance(row.get('_id'), str) for row in rows)
                and len({row['_id'] for row in rows}) == len(rows), 'invalid document identities')
    launches = {row['_id']: row for row in collections['trigger_launches']}
    protocols = {row['_id']: row for row in collections['trigger_launch_input_protocols']}
    require(set(launches) == set(protocols) == set(saved), 'API and durable admission IDs differ')
    for key, launch in launches.items():
        record, protocol = launch['record'], protocols[key]
        require(integer(launch['document_schema_version']) == 1 and record['id'] == key
                and record['authority'] == 'visitor' and record['state'] == 'draft'
                and launch['owner'] == OWNER, 'stored Launch is not a draft visitor admission')
        require(set(protocol) == {'_id', 'document_schema_version', 'owner', 'public_id', 'command_key_hash', 'record'}
                and integer(protocol['document_schema_version']) == 1, 'input receipt shape differs')
        require(all(protocol[k] == launch[k] for k in ('owner', 'public_id', 'command_key_hash')),
                'input receipt identity differs')
        selected = protocol['record']
        require(selected == launch.get('trigger_input_protocol') and set(selected) == {
            'schema_version', 'launch_id', 'command_fingerprint', 'created_at', 'initiation_sha256'},
                'input protocol differs from retained Launch')
        require(integer(selected['schema_version']) == 1 and selected['launch_id'] == key
                and selected['command_fingerprint'] == record['command_fingerprint']
                and selected['created_at'] == record['created_at']
                and re.fullmatch(r'[a-f0-9]{64}', selected['initiation_sha256']), 'input protocol binding differs')
    quota = collections['visitor_launch_days'][0]
    require(set(quota) == {'_id', 'document_schema_version', 'used', 'expires_at'}
            and quota['_id'] == str(day) + ':' + OWNER and integer(quota['document_schema_version']) == 1
            and integer(quota['used']) == expected_count
            and quota['expires_at'] == {'$date': {'$numberLong': str((day + 2) * 86400000)}},
            'retained quota count, schema or expiry differs')
    return collections


def compare(before, after, writer):
    for name in COLLECTIONS:
        old = {row['_id']: row for row in before[name]}
        new = {row['_id']: row for row in after[name]}
        if not writer:
            require(new == old, 'retained reader changed durable ' + name)
        elif name != 'visitor_launch_days':
            require(all(new.get(key) == row for key, row in old.items()) and len(new) == len(old) + 2,
                    'new writer rewrote old admission or created wrong count')
        else:
            require(set(new) == set(old), 'writer changed quota identity')
            for key, row in old.items():
                require({k: v for k, v in row.items() if k != 'used'} ==
                        {k: v for k, v in new[key].items() if k != 'used'}
                        and integer(new[key]['used']) == integer(row['used']) + 2,
                        'writer changed quota beyond two admissions')


def capture(campaign, namespace, image, stage, source, application_image):
    require(stage in STAGES, 'unknown document capture stage')
    manifest = job(namespace, image, stage)
    name = manifest['metadata']['name']
    campaign.kubectl(['apply', '-f', '-'], input_text=json.dumps(manifest))
    try:
        campaign.kubectl(['-n', namespace, 'wait', '--for=condition=complete', 'job/' + name,
                          '--timeout=125s'], timeout=135)
    except Exception:
        # Preserve the original wait failure; collect the bounded, credential-free
        # Job output before the outer runner removes the disposable namespace.
        try:
            raw = campaign.kubectl(['-n', namespace, 'logs', 'job/' + name,
                                    '-c', 'launch-documents', '--tail=30'], timeout=20)
            (campaign.output / ('launch-documents-' + stage + '.failed.log')).write_text(raw)
        except Exception as error:
            (campaign.output / ('launch-documents-' + stage + '.diagnostic-error.json')).write_text(
                json.dumps({'type': type(error).__name__, 'message': 'failed Job log unavailable'}) + '\n')
        raise
    raw = campaign.kubectl(['-n', namespace, 'logs', 'job/' + name, '-c', 'launch-documents'], timeout=20)
    path = campaign.output / ('launch-documents-' + stage + '.private.json')
    path.write_text(raw)
    data = json.loads(raw)
    writers = ('old-write',) if STAGES.index(stage) < 2 else ('old-write', 'new-write')
    records = [json.loads((campaign.output / (writer + '-visitor.private.json')).read_text()) for writer in writers]
    rows = validate(data, records, 2 * len(writers))
    index = STAGES.index(stage)
    if index:
        previous = json.loads((campaign.output / ('launch-documents-' + STAGES[index - 1] + '.private.json')).read_text())
        compare(previous['collections'], rows, stage == 'new-write')
    return {'stage': stage, 'source': source, 'image': application_image,
            'path': path.name, 'sha256': hashlib.sha256(path.read_bytes()).hexdigest(),
            'collections': list(COLLECTIONS), 'launches': 2 * len(writers), 'outcome': 'PASS'}
