"""Preserve exact OCI manifests when importing an owned disposable kind node.

Docker-save conversion can produce a different manifest. This importer accepts
only an OCI archive whose original manifest/config/layer bytes are verified,
and checks the same bytes plus CRI identity after kind imports it. No image is
retagged to make a changed manifest appear to match a registry identity.
"""
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import tarfile

from migration_native_dependencies import IMAGE, owned_cluster, runtime_identity

MAX_ARCHIVE = 40 * 1024**3
MAX_METADATA = 1024**2
MANIFEST_TYPES = ('application/vnd.oci.image.manifest.v1+json',
                  'application/vnd.docker.distribution.manifest.v2+json')


class ImageImportError(RuntimeError):
    pass


def require(value, reason):
    if not value:
        raise ImageImportError(reason)


def sha256(data):
    return 'sha256:' + hashlib.sha256(data).hexdigest()


def descriptor(value):
    require(isinstance(value, dict) and isinstance(value.get('digest'), str)
            and re.fullmatch('sha256:[0-9a-f]{64}', value['digest'])
            and type(value.get('size')) is int and 0 < value['size'] <= MAX_ARCHIVE,
            'native_oci_descriptor_invalid')
    return 'blobs/sha256/' + value['digest'].split(':')[1]


def validate_archive(path, expected):
    """Validate all referenced bytes without extracting files or starting tools."""
    require(isinstance(expected, dict) and set(expected) in (
        {'image', 'config'}, {'image', 'config', 'source'}), 'native_oci_identity_invalid')
    require(isinstance(expected['image'], str) and IMAGE.fullmatch(expected['image'])
            and isinstance(expected['config'], str)
            and re.fullmatch('sha256:[a-f0-9]{64}', expected['config']), 'native_oci_identity_invalid')
    path = Path(path)
    require(not path.is_symlink() and path.is_file(), 'native_oci_regular_archive_required')
    descriptor_fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    with os.fdopen(descriptor_fd, 'rb') as stream:
        before = os.fstat(stream.fileno())
        require(stat.S_ISREG(before.st_mode) and 0 < before.st_size <= MAX_ARCHIVE,
                'native_oci_archive_bound')
        with tarfile.open(fileobj=stream, mode='r:') as archive:
            members = []
            for member in archive:
                members.append(member)
                require(len(members) <= 260, 'native_oci_member_bound')
            require(bool(members), 'native_oci_member_bound')
            names = set()
            for member in members:
                require(member.name not in names and (member.isfile() or member.isdir())
                        and (member.name in ('oci-layout', 'index.json', 'blobs', 'blobs/sha256')
                             or re.fullmatch('blobs/sha256/[a-f0-9]{64}', member.name)),
                        'native_oci_member_invalid')
                require(member.isfile() or member.name in ('blobs', 'blobs/sha256'),
                        'native_oci_directory_invalid')
                names.add(member.name)
            def metadata(name):
                item = archive.getmember(name)
                require(item.isfile() and 0 < item.size <= MAX_METADATA, 'native_oci_metadata_bound')
                return archive.extractfile(item).read()
            require(json.loads(metadata('oci-layout')) == {'imageLayoutVersion': '1.0.0'},
                    'native_oci_layout_invalid')
            index = json.loads(metadata('index.json'))
            require(index.get('schemaVersion') == 2 and isinstance(index.get('manifests'), list)
                    and len(index['manifests']) == 1, 'native_oci_single_platform_required')
            selected = index['manifests'][0]
            manifest_path = descriptor(selected)
            require(selected.get('mediaType') in MANIFEST_TYPES
                    and selected['digest'] == expected['image'].split('@')[1]
                    and selected.get('annotations', {}).get('io.containerd.image.name') == expected['image']
                    and selected.get('platform', {'os': 'linux', 'architecture': 'amd64'})
                    == {'os': 'linux', 'architecture': 'amd64'}, 'native_oci_image_binding_invalid')
            manifest_raw = metadata(manifest_path)
            manifest = json.loads(manifest_raw)
            require(manifest.get('schemaVersion') == 2 and manifest.get('mediaType') in MANIFEST_TYPES
                    and manifest.get('config', {}).get('digest') == expected['config']
                    and isinstance(manifest.get('layers'), list) and 1 <= len(manifest['layers']) <= 240,
                    'native_oci_manifest_invalid')
            config_path = descriptor(manifest['config'])
            config_raw = metadata(config_path)
            config = json.loads(config_raw)
            require(config.get('os') == 'linux' and config.get('architecture') == 'amd64',
                    'native_oci_platform_invalid')
            if 'source' in expected:
                runtime_identity(expected, manifest_raw, config_raw)
            descriptors = [selected, manifest['config'], *manifest['layers']]
            expected_names = {'oci-layout', 'index.json'}
            for item in descriptors:
                name = descriptor(item)
                expected_names.add(name)
                entry = archive.getmember(name)
                require(entry.isfile() and entry.size == item['size'], 'native_oci_blob_size_mismatch')
                checksum = hashlib.sha256()
                with archive.extractfile(entry) as blob:
                    for chunk in iter(lambda: blob.read(1024**2), b''):
                        checksum.update(chunk)
                require('sha256:' + checksum.hexdigest() == item['digest'], 'native_oci_blob_digest_mismatch')
            require(names - {'blobs', 'blobs/sha256'} == expected_names, 'native_oci_unreferenced_blob')
        stream.seek(0)
        checksum = hashlib.sha256()
        for chunk in iter(lambda: stream.read(1024**2), b''):
            checksum.update(chunk)
        after = os.fstat(stream.fileno())
        require((before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns)
                == (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns),
                'native_oci_archive_changed')
    return {'schema': 'migration-oci-archive-v1', 'artifact': dict(expected),
            'archive_sha256': checksum.hexdigest(), 'archive_bytes': after.st_size,
            'manifest_sha256': sha256(manifest_raw), 'config_sha256': sha256(config_raw),
            'layers_verified': len(manifest['layers']), 'import_status': 'NOT_RUN'}


def verify_node(command, context, cluster, node):
    owned_cluster(cluster, node)
    require(isinstance(context, str) and re.fullmatch('[a-zA-Z0-9_-]{1,80}', context),
            'native_local_docker_context_required')
    details = json.loads(command(['docker', 'context', 'inspect', context]))
    require(len(details) == 1 and details[0].get('Name') == context
            and details[0].get('Endpoints', {}).get('docker', {}).get('Host', '').startswith('unix:///'),
            'native_local_docker_endpoint_required')
    rows = json.loads(command(['docker', '--context', context, 'inspect', node]))
    require(len(rows) == 1, 'native_owned_node_missing')
    row = rows[0]
    require(re.fullmatch('[a-f0-9]{64}', row.get('Id', '')) and row.get('Name') == '/' + node
            and row.get('Config', {}).get('Labels', {}).get('io.x-k8s.kind.cluster') == cluster
            and row.get('State', {}).get('Running') is True, 'native_owned_node_mismatch')
    return row['Id']


def import_archive(command, *, path, expected, context, cluster, owner_node_id,
                   kind='kind', enabled=False):
    require(enabled is True, 'native_image_import_explicit_opt_in_required')
    node = cluster + '-control-plane'
    require(isinstance(owner_node_id, str) and re.fullmatch('[a-f0-9]{64}', owner_node_id),
            'native_created_node_custody_required')
    archive = validate_archive(path, expected)
    node_id = verify_node(command, context, cluster, node)
    require(node_id == owner_node_id, 'native_created_node_custody_mismatch')
    # kind inherits this command's explicit environment only. No global Docker
    # context change, registry read, source rebuild or layer conversion occurs.
    command(['env', 'DOCKER_CONTEXT=' + context, 'KIND_EXPERIMENTAL_PROVIDER=docker',
             kind, 'load', 'image-archive', '--name', cluster, str(Path(path).resolve())])
    require(verify_node(command, context, cluster, node) == node_id, 'native_owned_node_replaced')
    prefix = ['docker', '--context', context, 'exec', node_id]
    for key, expected_digest in [('manifest', expected['image'].split('@')[1]), ('config', expected['config'])]:
        raw = command([*prefix, 'ctr', '--namespace', 'k8s.io', 'content', 'get', expected_digest])
        require(isinstance(raw, bytes) and len(raw) <= MAX_METADATA and sha256(raw) == expected_digest,
                'native_imported_' + key + '_mismatch')
    cri = json.loads(command([*prefix, 'crictl', 'inspecti', expected['image']]))
    status = cri.get('status', {})
    require(status.get('id') == expected['config']
            and expected['image'] in status.get('repoDigests', []), 'native_imported_cri_mismatch')
    require(verify_node(command, context, cluster, node) == node_id, 'native_owned_node_replaced')
    return {**archive, 'import_status': 'EXACT_IMAGE_IMPORTED', 'cluster': cluster,
            'node_container_id': node_id, 'cri_config': status['id'],
            'cri_repo_digests': status['repoDigests'], 'manifest_preserved': True}
