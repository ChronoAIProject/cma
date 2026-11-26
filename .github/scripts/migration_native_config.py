"""Offline native campaign input binding; no runtime starts during validation."""
import hashlib
import json
import os
from pathlib import Path
import re
import ssl
import tarfile

from migration_kind_manifests import IDENTITY
from migration_native_campaign import artifact
from migration_native_dependencies import ASSETS, ASSET_MANIFEST_SHA256, IMAGE, read_pinned
from migration_native_endpoints import TLS_PINS
from migration_native_process import require


def regular(path, limit=None, private=False):
    require(isinstance(path, str) and Path(path).is_absolute(), 'native_absolute_input_required')
    result = Path(path)
    require(result.resolve() == result and result.is_file() and not result.is_symlink()
            and result.stat().st_uid == os.getuid(), 'native_regular_owned_input_required')
    if private:
        require(not result.stat().st_mode & 0o077, 'native_private_input_required')
    if limit:
        require(0 < result.stat().st_size <= limit, 'native_input_size_bound')
    return result


def dependency(value):
    require(isinstance(value, dict) and set(value) == {'image', 'config'}
            and isinstance(value['image'], str) and IMAGE.fullmatch(value['image'])
            and isinstance(value['config'], str) and re.fullmatch('sha256:[a-f0-9]{64}', value['config']),
            'native_dependency_identity_invalid')
    return dict(value)


def load(path):
    config_path = regular(str(path), 1024**2, True)
    value = json.loads(config_path.read_bytes())
    require(isinstance(value, dict) and set(value) == {
        'schema', 'artifacts', 'mongo', 'identity', 'node', 'docker_context', 'archives', 'tls', 'inventory'},
        'native_input_shape_invalid')
    require(value['schema'] == 'migration-native-input-v1', 'native_input_schema_invalid')
    require(isinstance(value['artifacts'], dict) and set(value['artifacts']) == {'old', 'new', 'runtime'},
            'native_input_artifacts_required')
    artifacts = {key: artifact(row) for key, row in value['artifacts'].items()}
    require(artifacts['old']['image'] != artifacts['new']['image'], 'native_distinct_readers_required')
    mongo, identity = dependency(value['mongo']), dependency(value['identity'])
    require(mongo['image'].startswith('docker.io/library/mongo@sha256:')
            and identity['image'] == IDENTITY, 'native_fixture_dependency_mismatch')
    node = value['node']
    require(isinstance(node, dict) and set(node) == {'image', 'config', 'architecture'}
            and node['architecture'] in ('amd64', 'arm64'), 'native_node_identity_invalid')
    dependency({k: node[k] for k in ('image', 'config')})
    require(node['image'].startswith('docker.io/kindest/node@sha256:'), 'native_kind_node_required')
    require(isinstance(value['docker_context'], str)
            and re.fullmatch('[a-zA-Z0-9_-]{1,80}', value['docker_context']), 'native_docker_context_invalid')
    pinned = read_pinned(ASSETS, 'manifest.json', ASSET_MANIFEST_SHA256)['images']
    images = {**artifacts, 'mongo': mongo, 'identity': identity,
              **{name: {key: row[key] for key in ('image', 'config')} for name, row in pinned.items()}}
    require(isinstance(value['archives'], dict)
            and set(value['archives']) == {row['image'] for row in images.values()},
            'native_archive_set_mismatch')
    for archive in value['archives'].values(): regular(archive, 40*1024**3)
    tls = value['tls']
    require(isinstance(tls, dict) and set(tls) == set(TLS_PINS), 'native_tls_pair_required')
    for name, tls_path in tls.items():
        raw = regular(tls_path, 65536, name == 'private_key').read_bytes()
        require(hashlib.sha256(raw).hexdigest() == TLS_PINS[name], 'native_tls_pin_mismatch')
    inventory = json.loads(regular(value['inventory'], 4*1024**2).read_bytes())
    require(inventory.get('schema') == 'cma-reader-writer-source-inventory-v1'
            and inventory.get('old', {}).get('source') == artifacts['old']['source']
            and inventory.get('new', {}).get('source') == artifacts['new']['source']
            and isinstance(inventory.get('collections'), list) and bool(inventory['collections']),
            'native_source_inventory_mismatch')
    from migration_agent_documents import COLLECTIONS
    names = [row['name'] for row in inventory['collections']]
    require(len(set(names)) == len(names) and set(COLLECTIONS) <= set(names), 'native_inventory_scope_missing')
    return {**value, 'artifacts': artifacts, 'images': images,
            'input_sha256': hashlib.sha256(config_path.read_bytes()).hexdigest(),
            'inventory_collections': len(names)}


def validate_tls(tls):
    """Verify the supplied pair in memory; cluster probes require system trust.

    Loading the pinned certificate as a local trust anchor checks key, hostname
    and lifetime without opening a port or changing the machine's trust store.
    The separate in-cluster probe does not receive this trust override.
    """
    server = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    server.load_cert_chain(tls['certificate'], tls['private_key'])
    client = ssl.create_default_context()
    client.load_verify_locations(cafile=tls['certificate'])
    import time
    for name in ('api.localhost.direct', 'provider.localhost.direct'):
        ci, co, si, so = (ssl.MemoryBIO() for _ in range(4))
        peers = (client.wrap_bio(ci, co, server_hostname=name), server.wrap_bio(si, so, server_side=True))
        complete = [False, False]
        for _ in range(16):
            for index, peer in enumerate(peers):
                if complete[index]: continue
                try: peer.do_handshake();complete[index] = True
                except ssl.SSLWantReadError: pass  # Exchange only this pair's pending handshake bytes.
            if co.pending: si.write(co.read())
            if so.pending: ci.write(so.read())
            if all(complete): break
        require(all(complete), 'native_tls_handshake_incomplete')
        certificate = peers[0].getpeercert()
        require(ssl.cert_time_to_seconds(certificate['notAfter']) > time.time()+7200,
                'native_tls_validity_window_invalid')


def metadata_from_archive(path, image, config):
    """Call only after validate_archive/import_archive checked the complete file."""
    with tarfile.open(path, mode='r:') as archive:
        result = []
        for digest in (image.split('@')[1], config):
            entry = archive.getmember('blobs/sha256/' + digest.split(':')[1])
            require(entry.isfile() and 0 < entry.size <= 1024**2, 'native_metadata_bound')
            raw = archive.extractfile(entry).read()
            require('sha256:' + hashlib.sha256(raw).hexdigest() == digest, 'native_metadata_changed')
            result.append(raw)
        return result
