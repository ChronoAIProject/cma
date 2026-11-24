"""Offline binding of pinned infrastructure for one disposable native campaign."""
from copy import deepcopy
import hashlib
import json
from pathlib import Path
import re

ASSET_MANIFEST_SHA256 = 'a8d3ff7275ba363ea5b1ecfa001503f21047b7d0fba8e3432d632a99326881c6'
ASSETS = Path(__file__).with_name('migration-native-assets')
IMAGE = re.compile(r'[a-z0-9][a-z0-9./:_-]*@sha256:[0-9a-f]{64}')
CLUSTER = re.compile(r'cma-matrix-[0-9a-f]{10}')


def digest(data):
    return hashlib.sha256(data).hexdigest()


def read_pinned(directory, name, expected):
    path = directory / name
    if path.is_symlink() or not path.is_file() or path.stat().st_size > 1024 * 1024:
        raise ValueError('native_dependency_file_invalid')
    data = path.read_bytes()
    if digest(data) != expected:
        raise ValueError('native_dependency_hash_mismatch')
    return json.loads(data)


def object_key(obj):
    try:
        return (obj['apiVersion'], obj['kind'], obj['metadata'].get('namespace', ''),
                obj['metadata']['name'])
    except (KeyError, TypeError, AttributeError) as error:
        raise ValueError('native_object_identity_invalid') from error


def unique_objects(objects):
    keys = [object_key(obj) for obj in objects]
    if len(set(keys)) != len(keys):
        raise ValueError('native_duplicate_object')


def owned_cluster(cluster, node):
    if not isinstance(cluster, str) or not CLUSTER.fullmatch(cluster):
        raise ValueError('native_owned_cluster_required')
    if node != cluster + '-control-plane':
        raise ValueError('native_owned_node_required')


def runtime_identity(identity, manifest_bytes, config_bytes):
    """Verify raw OCI bytes, rather than accepting a caller's PASS boolean."""
    if (not isinstance(identity, dict) or set(identity) != {'image', 'source', 'config'}
            or not isinstance(identity['image'], str) or not IMAGE.fullmatch(identity['image'])
            or not isinstance(identity['source'], str)
            or not re.fullmatch('[0-9a-f]{40}', identity['source'])
            or not isinstance(identity['config'], str)
            or not re.fullmatch('sha256:[0-9a-f]{64}', identity['config'])):
        raise ValueError('native_runtime_identity_invalid')
    if any(not isinstance(value, bytes) or len(value) > 1024 * 1024
           for value in (manifest_bytes, config_bytes)):
        raise ValueError('native_runtime_raw_metadata_required')
    if ('sha256:' + digest(manifest_bytes) != identity['image'].split('@')[1]
            or 'sha256:' + digest(config_bytes) != identity['config']):
        raise ValueError('native_runtime_digest_mismatch')
    try:
        manifest, config = json.loads(manifest_bytes), json.loads(config_bytes)
        if (manifest['schemaVersion'] != 2
                or manifest['config']['digest'] != identity['config']
                or manifest['config']['size'] != len(config_bytes)
                or config['os'] != 'linux' or config['architecture'] != 'amd64'
                or config['config']['Labels']['org.opencontainers.image.revision'] != identity['source']):
            raise ValueError('native_runtime_metadata_mismatch')
    except (KeyError, TypeError, json.JSONDecodeError) as error:
        raise ValueError('native_runtime_metadata_invalid') from error
    return deepcopy(identity)


def dependencies(cluster, node, *, directory=ASSETS):
    """Render only; import and live controller/CSI checks belong to the runner."""
    owned_cluster(cluster, node)
    directory = Path(directory)
    manifest = read_pinned(directory, 'manifest.json', ASSET_MANIFEST_SHA256)
    objects = []
    for entry in manifest['objects']:
        obj = read_pinned(directory, entry['file'], entry['sha256'])
        objects.append(obj)
    unique_objects(objects)
    for obj in objects:
        if obj['kind'] not in ('Deployment', 'StatefulSet'):
            continue
        spec = obj['spec']['template']['spec']
        spec['nodeSelector'] = {'kubernetes.io/hostname': node}
        for container in spec['containers']:
            container['image'] = manifest['images'][container['name']]['image']
            container['imagePullPolicy'] = 'Never'
    return objects, {
        'asset_manifest_sha256': ASSET_MANIFEST_SHA256,
        'images': deepcopy(manifest['images']),
        'controller_release': deepcopy(manifest['controller_release']),
        'exclusive_disposable_cluster_required': True,
        'cluster_scoped_authority': 'controller and CSI only; never a shared cluster',
        'import_status': 'NOT_RUN', 'readiness_status': 'NOT_PROVEN',
    }
