"""Collect and compare complete Mongo catalogs across immutable reader switches.

Catalog preservation is a separate observation from document compatibility.
It cannot certify that an old application understands a new persisted shape.
"""
import hashlib
import json
from pathlib import Path


SCRIPT = Path(__file__).with_suffix('.cjs')
SCHEMA = 'cma-migration-catalog-v1'
STAGES = ('old-write', 'new-read-old-write', 'new-write', 'old-read-new-write')


class CatalogError(RuntimeError):
    pass


def validate(value):
    if (not isinstance(value, dict) or set(value) != {'schema', 'database', 'collections'}
            or value['schema'] != SCHEMA or value['database'] != 'matrix'):
        raise CatalogError('invalid catalog identity')
    rows = value['collections']
    if not isinstance(rows, list) or not 1 <= len(rows) <= 512:
        raise CatalogError('missing or excessive catalog collections')
    result = {}
    for row in rows:
        if (not isinstance(row, dict)
                or set(row) != {'name', 'type', 'uuid', 'options', 'indexes'}
                or not isinstance(row['name'], str) or not row['name']
                or row['name'] in result
                or row['type'] not in ('collection', 'view', 'timeseries')
                or not isinstance(row['options'], dict)):
            raise CatalogError('invalid catalog collection')
        if row['type'] == 'collection' and not row['uuid']:
            raise CatalogError('collection UUID missing')
        indexes = row['indexes']
        if not isinstance(indexes, list) or len(indexes) > 256:
            raise CatalogError('invalid catalog indexes')
        if row['type'] != 'view' and not indexes:
            raise CatalogError('collection indexes missing')
        names = set()
        for index in indexes:
            if (not isinstance(index, dict) or 'spec' in index or 'buildUUID' in index
                    or not isinstance(index.get('name'), str) or not index['name']
                    or index['name'] in names):
                raise CatalogError('invalid or unfinished catalog index')
            keys = index.get('key')
            if (not isinstance(keys, list) or not keys
                    or any(not isinstance(pair, list) or len(pair) != 2
                           or not isinstance(pair[0], str) or not pair[0] for pair in keys)
                    or len({pair[0] for pair in keys}) != len(keys)):
                raise CatalogError('index key order missing or ambiguous')
            names.add(index['name'])
        result[row['name']] = row
    return result


def compare(before, after):
    """Require every existing definition and collection identity to survive."""
    old, new = validate(before), validate(after)
    changes, additions = [], []
    for name, previous in old.items():
        if name not in new:
            changes.append({'collection': name, 'change': 'collection_removed'})
            continue
        current = new[name]
        for field in ('type', 'uuid', 'options'):
            if previous[field] != current[field]:
                changes.append({'collection': name, 'change': field + '_changed'})
        old_indexes = {index['name']: index for index in previous['indexes']}
        new_indexes = {index['name']: index for index in current['indexes']}
        for key, index in old_indexes.items():
            if new_indexes.get(key) != index:
                changes.append({'collection': name, 'index': key,
                                'change': 'index_removed_or_changed'})
        additions.extend({'collection': name, 'index': key, 'change': 'index_added'}
                         for key in new_indexes if key not in old_indexes)
    additions.extend({'collection': name, 'change': 'collection_added'}
                     for name in new if name not in old)
    return {'outcome': 'PRESERVED' if not changes else 'CHANGED', 'changes': changes,
            'additions_requiring_review': additions, 'compared_collections': len(old)}


def infrastructure(objects, namespace, password):
    """Give the collector catalog-only privileges, without document access."""
    init = next(obj for obj in objects if obj['kind'] == 'Job'
                and obj['metadata']['name'] == 'mongo-init')
    init['spec']['template']['spec']['containers'][0]['env'].append(
        {'name': 'MATRIX_CATALOG_PASSWORD', 'value': password})
    code = next(obj for obj in objects if obj['kind'] == 'ConfigMap'
                and obj['metadata']['name'] == 'mongo-init')
    code['data']['init.sh'] += '''
mongosh --host mongo:27017 --username matrix_admin --password "$MATRIX_ADMIN_PASSWORD" --authenticationDatabase admin --quiet --eval 'const m=db.getSiblingDB("matrix"); m.createRole({role:"matrix_catalog",privileges:[{resource:{db:"matrix",collection:""},actions:["listCollections","listIndexes","killCursors"]}],roles:[]}); m.createUser({user:"matrix_catalog",pwd:process.env.MATRIX_CATALOG_PASSWORD,roles:[{role:"matrix_catalog",db:"matrix"}]})'
'''
    uri = (f'mongodb://matrix_catalog:{password}@mongo:27017/matrix?authSource=matrix'
           '&replicaSet=matrix&serverSelectionTimeoutMS=5000&connectTimeoutMS=5000'
           '&socketTimeoutMS=5000')
    objects.extend([
        {'apiVersion': 'v1', 'kind': 'ConfigMap',
         'metadata': {'name': 'mongo-catalog-code', 'namespace': namespace},
         'data': {'read.cjs': SCRIPT.read_text()}},
        {'apiVersion': 'v1', 'kind': 'ConfigMap',
         'metadata': {'name': 'mongo-catalog-auth', 'namespace': namespace},
         'data': {'MATRIX_CATALOG_URI': uri}},
    ])
    return objects


def job(namespace, image, stage):
    if stage not in STAGES:
        raise CatalogError('unknown catalog stage')
    return {'apiVersion': 'batch/v1', 'kind': 'Job',
            'metadata': {'name': 'catalog-' + stage, 'namespace': namespace},
            'spec': {'backoffLimit': 0, 'activeDeadlineSeconds': 120,
                     'template': {'spec': {
                         'automountServiceAccountToken': False, 'restartPolicy': 'Never',
                         'containers': [{'name': 'catalog', 'image': image,
                                         'imagePullPolicy': 'Never',
                                         'command': ['mongosh', '--nodb', '--quiet', '/catalog/read.cjs'],
                                         'envFrom': [{'configMapRef': {'name': 'mongo-catalog-auth'}}],
                                         'volumeMounts': [{'name': 'code', 'mountPath': '/catalog',
                                                           'readOnly': True}],
                                         'resources': {'requests': {'cpu': '50m', 'memory': '128Mi'},
                                                       'limits': {'cpu': '250m', 'memory': '256Mi'}}}],
                         'volumes': [{'name': 'code', 'configMap': {'name': 'mongo-catalog-code'}}]}}}}


def capture(campaign, namespace, image, stage, source, application_image):
    manifest = job(namespace, image, stage)
    name = manifest['metadata']['name']
    campaign.kubectl(['apply', '-f', '-'], input_text=json.dumps(manifest))
    campaign.kubectl(['-n', namespace, 'wait', '--for=condition=complete',
                      'job/' + name, '--timeout=125s'], timeout=135)
    raw = campaign.kubectl(['-n', namespace, 'logs', 'job/' + name, '-c', 'catalog'], timeout=20)
    try:
        data = json.loads(raw)
    except json.JSONDecodeError as error:
        raise CatalogError('catalog output missing or malformed') from error
    validate(data)
    path = campaign.output / ('catalog-' + stage + '.private.json')
    path.write_text(raw)
    return {'stage': stage, 'source': source, 'image': application_image,
            'path': path.name, 'sha256': hashlib.sha256(path.read_bytes()).hexdigest(),
            'collections': len(data['collections'])}, data
