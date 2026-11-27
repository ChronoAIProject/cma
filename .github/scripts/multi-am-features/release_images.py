"""Validate and import an explicitly frozen local release image bundle."""
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import tarfile
from artifacts import inspect_imported_image

COMPONENTS = ('api', 'runtime', 'runtime-omarchy')
SOURCE = 'https://github.com/ChronoAIProject/cma'


def require(condition, code):
    if not condition:
        raise ValueError(code)


def file_hash(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def regular_child(root, name):
    require(isinstance(name, str) and Path(name).name == name and name not in ('', '.', '..'),
            'release_archive_path')
    path = root / name
    require(path.is_file() and not path.is_symlink(), 'release_archive_regular_file_required')
    return path


def check_archive(path, row, component):
    require(path.stat().st_size == row.get('archiveBytes') and path.stat().st_size > 0,
            'release_archive_size')
    require(file_hash(path) == row.get('archiveSha256'), 'release_archive_hash')
    with tarfile.open(path, 'r:*') as archive:
        members = archive.getmembers()
        require(len(members) <= 10000 and len({x.name for x in members}) == len(members),
                'release_archive_members')
        require(all(not PurePosixPath(member.name).is_absolute()
                    and '..' not in PurePosixPath(member.name).parts
                    and (member.isfile() or member.isdir()) for member in members),
                'release_archive_unsafe_member')
        by_name = {x.name: x for x in members}

        def read(name):
            member = by_name.get(name)
            require(member is not None and member.isfile() and member.size <= 1024 * 1024,
                    'release_metadata_member')
            return archive.extractfile(member).read()

        def blob(digest):
            raw = read('blobs/sha256/' + digest.removeprefix('sha256:'))
            require('sha256:' + hashlib.sha256(raw).hexdigest() == digest, 'release_blob_hash')
            return raw, json.loads(raw)

        index = json.loads(read('index.json'))
        require(index.get('schemaVersion') == 2 and len(index.get('manifests', [])) == 1
                and index['manifests'][0].get('digest') == row['manifest'],
                'release_single_platform_manifest_required')
        raw_manifest, manifest = blob(row['manifest'])
        require(len(raw_manifest) == index['manifests'][0].get('size')
                and manifest.get('schemaVersion') == 2
                and manifest.get('config', {}).get('digest') == row['config'],
                'release_manifest_config_mismatch')
        raw_config, config = blob(row['config'])
        require(len(raw_config) == manifest['config'].get('size'), 'release_config_size')
        require(config.get('os') == 'linux' and config.get('architecture') == 'amd64',
                'release_config_platform')
        labels = config.get('config', {}).get('Labels', {}) or {}
        require(labels.get('org.opencontainers.image.source') == SOURCE
                and labels.get('org.opencontainers.image.revision') == row['sourceRevision'],
                'release_image_source_labels')
        if component.startswith('runtime'):
            require(labels.get('dev.chrono.cma.runtime-contract') == '3', 'release_runtime_contract')
        for layer in manifest.get('layers', []):
            digest = layer.get('digest', '')
            require(re.fullmatch(r'sha256:[0-9a-f]{64}', digest), 'release_layer_digest')
            member = by_name.get('blobs/sha256/' + digest.removeprefix('sha256:'))
            require(member is not None and member.isfile() and member.size == layer.get('size'),
                    'release_layer_size')
            require('sha256:' + hashlib.file_digest(archive.extractfile(member), 'sha256').hexdigest()
                    == digest, 'release_layer_hash')


def checked_packet(path, expected_sha256, revision):
    path = Path(path)
    require(path.is_file() and not path.is_symlink()
            and re.fullmatch('[0-9a-f]{64}', expected_sha256 or ''), 'release_packet_hash_required')
    require(file_hash(path) == expected_sha256, 'release_packet_hash')
    packet = json.loads(path.read_text())
    require(packet.get('schema') == 'multi-am-release-images-v1'
            and packet.get('apiSource') == revision, 'release_packet_source')
    images = packet.get('images', {})
    require(set(images) == set(COMPONENTS), 'release_packet_components')
    for component in COMPONENTS:
        row = images[component]
        require(row.get('platform') == 'linux/amd64'
                and re.fullmatch('[0-9a-f]{40}', row.get('sourceRevision', '')),
                'release_image_platform_source')
        for field in ('manifest', 'config'):
            require(re.fullmatch(r'sha256:[0-9a-f]{64}', row.get(field, '')), 'release_image_digest')
        if component == 'api':
            require(row['sourceRevision'] == revision, 'release_api_source_mismatch')
        archive = regular_child(path.parent, row.get('archive'))
        check_archive(archive, row, component)
    return packet


def load_release_image(command, node, cluster, directory, component, row):
    archive = regular_child(Path(directory), row['archive'])
    # Revalidate after preflight and before import; a mutable path cannot swap
    # the reviewed bytes while the cluster is being prepared.
    check_archive(archive, row, component)
    command(['kind', 'load', 'image-archive', '--name', cluster, str(archive)], timeout=300)
    prefix = ['docker', 'exec', node, 'ctr', '-n', 'k8s.io']
    rows = [line.split() for line in command(prefix + ['images', 'list']).splitlines()]
    matches = [parts for parts in rows if len(parts) >= 3 and parts[2] == row['manifest']]
    require(bool(matches), 'release_imported_manifest_missing')
    reference = f'localhost/multi-am-{component}@' + row['manifest']
    if not any(parts[0] == reference for parts in matches):
        command(prefix + ['images', 'tag', matches[0][0], reference])
    status = inspect_imported_image(command, node, reference)
    require(status.get('id') == row['config'], 'release_imported_config_mismatch')
    return reference
