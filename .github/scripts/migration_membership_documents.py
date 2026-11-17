"""Observe real coordinated membership during isolated image switches."""
from copy import deepcopy
import hashlib
import json
from pathlib import Path
import re

from migration_catalog import job as catalog_job
from migration_membership_validation import require, validate, MEMBERS, OWNERS
from migration_membership_history import begin, observations, retain


def infrastructure(objects, namespace, password):
    require(isinstance(password, str) and re.fullmatch(r'[A-Za-z0-9_-]{24,64}', password),
            'invalid local observer password encoding')
    selected = {(obj['kind'], obj['metadata']['name']): obj for obj in objects}
    init = selected[('Job', 'mongo-init')]['spec']['template']['spec']['containers'][0]
    init['env'].append({'name': 'MATRIX_MEMBERSHIP_PASSWORD', 'value': password})
    grants = [{'resource': {'db': 'matrix', 'collection': name}, 'actions': ['find']}
              for name in (MEMBERS, OWNERS)]
    script = ('const m=db.getSiblingDB("matrix"); m.createRole({role:"matrix_membership",'
              'privileges:' + json.dumps(grants, separators=(',', ':')) + ',roles:[]}); '
              'm.createUser({user:"matrix_membership",pwd:process.env.MATRIX_MEMBERSHIP_PASSWORD,'
              'roles:[{role:"matrix_membership",db:"matrix"}]})')
    selected[('ConfigMap', 'mongo-init')]['data']['init.sh'] += (
        '\nmongosh --host mongo:27017 --username matrix_admin --password "$MATRIX_ADMIN_PASSWORD" '
        '--authenticationDatabase admin --quiet --eval ' + "'" + script + "'\n")
    uri = (f'mongodb://matrix_membership:{password}@mongo:27017/matrix?authSource=matrix'
           '&replicaSet=matrix&serverSelectionTimeoutMS=5000&connectTimeoutMS=5000&socketTimeoutMS=5000')
    objects.extend([
        {'apiVersion': 'v1', 'kind': 'ConfigMap',
         'metadata': {'name': 'mongo-membership-code', 'namespace': namespace},
         'data': {'read.cjs': Path(__file__).with_suffix('.cjs').read_text()}},
        {'apiVersion': 'v1', 'kind': 'ConfigMap',
         'metadata': {'name': 'mongo-membership-auth', 'namespace': namespace},
         'data': {'MATRIX_MEMBERSHIP_READ_URI': uri}},
    ])
    return objects


def job(namespace, image, stage):
    result = deepcopy(catalog_job(namespace, image, stage))
    result['metadata']['name'] = 'membership-' + stage
    pod = result['spec']['template']['spec']
    pod['containers'][0]['name'] = 'membership'
    pod['containers'][0]['envFrom'] = [{'configMapRef': {'name': 'mongo-membership-auth'}}]
    pod['volumes'][0]['configMap']['name'] = 'mongo-membership-code'
    return result


def serving_identity(campaign):
    selected = campaign.active_reader['pod']
    pods = [p for p in campaign.reader_pods()['items']
            if p.get('metadata', {}).get('uid') == selected['uid']]
    require(len(pods) == 1, 'exact serving Pod missing')
    pod = pods[0]
    status = pod.get('status', {})
    containers = status.get('containerStatuses', [])
    require(len(containers) == 1, 'serving container count differs')
    container = containers[0]
    require(pod['metadata'].get('name') == selected['name']
            and not pod['metadata'].get('deletionTimestamp')
            and status.get('phase') == 'Running' and container.get('name') == 'api'
            and container.get('ready') is True and container.get('started') is True
            and container.get('restartCount') == 0
            and container.get('imageID') == selected['image_id']
            and isinstance(container.get('state', {}).get('running'), dict)
            and isinstance(container.get('containerID'), str) and container['containerID'],
            'serving Pod/container identity changed')
    return {'uid': selected['uid'], 'name': selected['name'], 'ip': status.get('podIP'),
            'image_id': selected['image_id'], 'container_id': container['containerID']}


def capture(campaign, namespace, image, stage, source, application_image):
    history = begin(campaign, stage, source, application_image)
    manifest = job(namespace, image, stage)
    pod = serving_identity(campaign)
    campaign.kubectl(['apply', '-f', '-'], input_text=json.dumps(manifest))
    name = manifest['metadata']['name']
    campaign.kubectl(['-n', namespace, 'wait', '--for=condition=complete',
                      'job/' + name, '--timeout=125s'], timeout=135)
    raw = campaign.kubectl(['-n', namespace, 'logs', 'job/' + name, '-c', 'membership'], timeout=20)
    path = campaign.output / ('membership-' + stage + '.private.json')
    path.write_text(raw)
    path.chmod(0o600)
    require(len(raw.encode()) <= 65536, 'snapshot exceeds byte bound')
    current = validate(json.loads(raw), pod)
    require(serving_identity(campaign) == pod, 'reader changed during membership capture')
    comparisons = observations(campaign, history, current)
    receipt = {'stage': stage, 'source': source, 'image': application_image,
               'path': path.name, 'sha256': hashlib.sha256(path.read_bytes()).hexdigest(),
               'membership_pod': pod, 'observed_collections': [MEMBERS],
               'empty_collections_required': [OWNERS], 'comparisons': comparisons,
               'outcome': 'OBSERVED', 'scope': 'COORDINATED_MEMBERSHIP_ONLY',
               'application_cross_reader_compatibility': 'NOT_PROVEN',
               'limitation': 'TTL may remove prior rows before another image reads them; '
                             'a Mongo observer is not an application decoder or owner transition.'}
    retain(campaign, history, receipt)
    return receipt
