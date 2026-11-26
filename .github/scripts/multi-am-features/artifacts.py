"""Verify the image exports produced by this CI invocation before loading them."""
import hashlib
import json
from pathlib import Path
import re
import stat
import subprocess

SOURCE = 'https://github.com/ChronoAIProject/cma'
MAX_METADATA_BYTES = 16 * 1024
MAX_ARCHIVE_BYTES = 8 * 1024 ** 3


def checked_metadata(directory, component, revision, run_id, attempt, *, source_tree):
    directory = Path(directory)
    manifest = directory / 'metadata.json'
    metadata_stat = manifest.lstat()
    if not stat.S_ISREG(metadata_stat.st_mode) or metadata_stat.st_size > MAX_METADATA_BYTES:
        raise ValueError('image_export_metadata_file')
    data = json.loads(manifest.read_text())
    expected = {'schema': 'cma-ci-native-image-v1', 'component': component,
                'sourceRevision': revision, 'sourceTree': source_tree,
                'platform': 'linux/amd64', 'source': SOURCE,
                'archiveFormat': 'docker-save+gzip'}
    if not isinstance(data, dict) or any(data.get(k) != v for k, v in expected.items()):
        raise ValueError('image_export_identity_mismatch')
    workflow = data.get('workflow')
    if (not isinstance(workflow, dict) or workflow.get('repository') != 'ChronoAIProject/cma'
            or workflow.get('runId') != run_id or workflow.get('runAttempt') != attempt):
        raise ValueError('image_export_run_mismatch')
    image_id = data.get('imageId')
    if not isinstance(image_id, str) or not re.fullmatch(r'sha256:[0-9a-f]{64}', image_id):
        raise ValueError('image_export_config_missing')
    version = data.get('versionLabel')
    if not isinstance(version, str) or not version.strip():
        raise ValueError('image_export_version_missing')
    name = f'cma-{component}-linux-amd64.docker.tar.gz'
    if data.get('archive') != name:
        raise ValueError('image_export_archive_path')
    archive = directory / name
    archive_stat = archive.lstat()
    size = data.get('archiveBytes')
    if (not stat.S_ISREG(archive_stat.st_mode) or type(size) is not int
            or not 0 < archive_stat.st_size == size <= MAX_ARCHIVE_BYTES):
        raise ValueError('image_export_archive_size')
    with archive.open('rb') as stream:
        if hashlib.file_digest(stream, 'sha256').hexdigest() != data.get('archiveSha256'):
            raise ValueError('image_export_archive_hash')
    if component.startswith('runtime') and data.get('runtimeContract') != '3':
        raise ValueError('image_export_runtime_contract')
    return data, archive


def load_image(command, node, cluster, directory, component, revision, run_id, attempt, *, source_tree):
    data, archive = checked_metadata(directory, component, revision, run_id, attempt, source_tree=source_tree)
    command(['docker', 'image', 'load', '--input', str(archive)], timeout=300)
    inspected = json.loads(command(['docker', 'image', 'inspect', data['imageId']]))
    if not isinstance(inspected, list) or len(inspected) != 1 or not isinstance(inspected[0], dict):
        raise ValueError('loaded_image_identity_mismatch')
    image = inspected[0]
    labels = (image.get('Config') or {}).get('Labels') or {}
    if (image.get('Id') != data['imageId'] or image.get('Architecture') != 'amd64'
            or image.get('Os') != 'linux' or labels.get('org.opencontainers.image.revision') != revision
            or labels.get('org.opencontainers.image.source') != SOURCE
            or labels.get('org.opencontainers.image.version') != data['versionLabel']
            or (component.startswith('runtime') and labels.get('dev.chrono.cma.runtime-contract') != '3')):
        raise ValueError('loaded_image_identity_mismatch')
    tag = f'localhost/multi-am-{component}:{revision}'
    command(['docker', 'image', 'tag', data['imageId'], tag])
    return import_tag(command, node, cluster, tag, data['imageId'])


def import_tag(command, node, cluster, tag, config_id):
    command(['kind', 'load', 'docker-image', '--name', cluster, tag], timeout=300)
    prefix = ['docker', 'exec', node, 'ctr', '-n', 'k8s.io']
    rows = command(prefix + ['images', 'list']).splitlines()
    matches = [row.split() for row in rows if row.split() and row.split()[0] == tag]
    if len(matches) != 1 or not re.fullmatch(r'sha256:[0-9a-f]{64}', matches[0][2]):
        raise ValueError('imported_manifest_missing')
    reference = tag.split(':')[0] + '@' + matches[0][2]
    command(prefix + ['images', 'tag', tag, reference])
    observed = json.loads(command(['docker', 'exec', node, 'crictl', 'inspecti', reference]))['status']
    if observed.get('id') != config_id:
        raise ValueError('imported_config_mismatch')
    return reference


def load_pinned_dependency(command, node, cluster, component, pinned):
    if (not re.fullmatch('[a-z][a-z0-9-]*', component)
            or not re.fullmatch(r'[^\s]+@sha256:[0-9a-f]{64}', pinned)):
        raise ValueError('pinned_dependency_required')
    command(['docker', 'pull', '--platform', 'linux/amd64', pinned], timeout=180)
    image = json.loads(command(['docker', 'image', 'inspect', pinned]))[0]
    if image.get('Architecture') != 'amd64' or image.get('Os') != 'linux':
        raise ValueError('dependency_image_platform_mismatch')
    if canonical_reference(pinned) not in {
            canonical_reference(ref) for ref in image.get('RepoDigests', [])}:
        raise ValueError('dependency_registry_pin_mismatch')
    if not re.fullmatch('sha256:[0-9a-f]{64}', image.get('Id', '')):
        raise ValueError('dependency_image_config_missing')
    # docker save/Kind import preserves the selected config, not necessarily the
    # registry's multi-platform index name. Bind the imported immutable manifest
    # back to the exact config pulled from the reviewed registry digest.
    tag = f'localhost/multi-am-{component}:' + pinned.rsplit(':', 1)[1]
    command(['docker', 'image', 'tag', image['Id'], tag])
    return import_tag(command, node, cluster, tag, image['Id'])


def canonical_reference(reference):
    # Docker may print docker.io/library/mongo as mongo in RepoDigests. Only
    # normalize the documented Docker Hub default; retain the exact digest.
    repository, separator, digest = reference.partition('@')
    if not separator or not re.fullmatch('sha256:[0-9a-f]{64}', digest):
        raise ValueError('pinned_dependency_required')
    parts = repository.split('/')
    if not all(parts) or any(part in ('.', '..') for part in parts):
        raise ValueError('pinned_dependency_required')
    first = parts[0]
    if len(parts) == 1 or ('.' not in first and ':' not in first and first != 'localhost'):
        parts.insert(0, 'docker.io')
    if parts[0] == 'docker.io' and len(parts) == 2:
        parts.insert(1, 'library')
    return '/'.join(parts) + '@' + digest
