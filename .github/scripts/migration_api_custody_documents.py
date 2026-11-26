"""Independent least-privilege document observation for API custody campaigns."""
from copy import deepcopy
import hashlib
import json
from pathlib import Path
import secrets

from migration_api_custody_validation import COLLECTION, compare, validate
from migration_catalog import job as catalog_job, STAGES
from migration_sandbox_documents import capture as capture_controls
from migration_sandbox_fixture import require


def infrastructure(objects):
    selected = {(o['kind'], o['metadata']['name']): o for o in objects}
    namespace = selected[('Deployment', 'api')]['metadata']['namespace']
    password = secrets.token_urlsafe(24)
    selected[('Job', 'mongo-init')]['spec']['template']['spec']['containers'][0]['env'].append(
        {'name': 'MATRIX_API_CUSTODY_READ_PASSWORD', 'value': password})
    script = ('const m=db.getSiblingDB("matrix");m.createRole({role:"matrix_api_custody_reader",'
              'privileges:[{resource:{db:"matrix",collection:"auth_api_bearers"},actions:["find"]}],roles:[]});'
              'm.createUser({user:"matrix_api_custody_reader",pwd:process.env.MATRIX_API_CUSTODY_READ_PASSWORD,'
              'roles:[{role:"matrix_api_custody_reader",db:"matrix"}]})')
    selected[('ConfigMap', 'mongo-init')]['data']['init.sh'] += (
        '\nmongosh --host mongo:27017 --username matrix_admin --password "$MATRIX_ADMIN_PASSWORD" '
        '--authenticationDatabase admin --quiet --eval ' + "'" + script + "'\n")
    uri = (f'mongodb://matrix_api_custody_reader:{password}@mongo:27017/matrix?authSource=matrix'
           '&replicaSet=matrix&serverSelectionTimeoutMS=5000&connectTimeoutMS=5000&socketTimeoutMS=5000')
    objects.extend([
        {'apiVersion': 'v1', 'kind': 'ConfigMap', 'metadata': {'name': 'mongo-api-custody-code', 'namespace': namespace},
         'data': {'read.cjs': Path(__file__).with_suffix('.cjs').read_text()}},
        {'apiVersion': 'v1', 'kind': 'ConfigMap', 'metadata': {'name': 'mongo-api-custody-auth', 'namespace': namespace},
         'data': {'MATRIX_API_CUSTODY_READ_URI': uri}},
    ])
    return objects


def job(namespace, image, stage):
    require(stage in STAGES, 'unknown API custody capture stage')
    result = deepcopy(catalog_job(namespace, image, stage))
    result['metadata']['name'] = 'api-custody-documents-' + stage
    pod = result['spec']['template']['spec']
    pod['containers'][0]['name'] = 'api-custody-documents'
    pod['containers'][0]['envFrom'] = [{'configMapRef': {'name': 'mongo-api-custody-auth'}}]
    pod['volumes'][0]['configMap']['name'] = 'mongo-api-custody-code'
    return result


def capture(campaign, namespace, image, stage, source, application_image):
    controls = capture_controls(campaign, namespace, image, stage, source, application_image)
    manifest = job(namespace, image, stage)
    name = manifest['metadata']['name']
    campaign.kubectl(['apply', '-f', '-'], input_text=json.dumps(manifest))
    try:
        campaign.kubectl(['-n', namespace, 'wait', '--for=condition=complete', 'job/' + name,
                          '--timeout=125s'], timeout=135)
    except Exception:
        try:
            raw = campaign.kubectl(['-n', namespace, 'logs', 'job/' + name,
                                    '-c', 'api-custody-documents', '--tail=30'], timeout=20)
            (campaign.output / (name + '.failed.log')).write_text(raw)
        except Exception as error:
            (campaign.output / (name + '.diagnostic-error.json')).write_text(
                json.dumps({'type': type(error).__name__, 'message': 'failed Job log unavailable'}) + '\n')
        raise
    raw = campaign.kubectl(['-n', namespace, 'logs', 'job/' + name, '-c', 'api-custody-documents'], timeout=20)
    path = campaign.output / (name + '.private.json')
    path.write_text(raw)
    data = json.loads(raw)
    writers = ('old-write',) if STAGES.index(stage) < 2 else ('old-write', 'new-write')
    records = [json.loads((campaign.output / (w + '-sandbox-controls.private.json')).read_text()) for w in writers]
    command_data = json.loads((campaign.output / controls['path']).read_text())
    rows = validate(data, records, command_data)
    index = STAGES.index(stage)
    if index:
        previous = json.loads((campaign.output / ('api-custody-documents-' + STAGES[index - 1]
                                                  + '.private.json')).read_text())
        compare(previous['collections'][COLLECTION], rows, stage == 'new-write')
    return {'stage': stage, 'source': source, 'image': application_image,
            'path': path.name, 'sha256': hashlib.sha256(path.read_bytes()).hexdigest(),
            'collections': [COLLECTION, *controls['collections']], 'custodies': len(rows),
            'command_snapshot': controls, 'outcome': 'PASS'}
