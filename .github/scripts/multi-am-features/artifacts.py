"""Verify the image exports produced by this CI invocation before loading them."""
import hashlib
import json
from pathlib import Path
import re
import subprocess


def checked_metadata(directory, component, revision, run_id, attempt):
    directory = Path(directory)
    data = json.loads((directory / 'metadata.json').read_text())
    expected = {'schema': 'cma-ci-native-image-v1', 'component': component,
                'sourceRevision': revision, 'platform': 'linux/amd64',
                'source': 'https://github.com/ChronoAIProject/cma',
                'archiveFormat': 'docker-save+gzip'}
    if any(data.get(k) != v for k, v in expected.items()):
        raise ValueError('image_export_identity_mismatch')
    workflow = data.get('workflow', {})
    if (workflow.get('repository') != 'ChronoAIProject/cma'
            or workflow.get('runId') != run_id or workflow.get('runAttempt') != attempt):
        raise ValueError('image_export_run_mismatch')
    if not re.fullmatch(r'sha256:[0-9a-f]{64}', data.get('imageId', '')):
        raise ValueError('image_export_config_missing')
    name = data.get('archive', '')
    if Path(name).name != name or not name.endswith('.docker.tar.gz'):
        raise ValueError('image_export_archive_path')
    archive = directory / name
    if archive.is_symlink() or archive.stat().st_size != data.get('archiveBytes'):
        raise ValueError('image_export_archive_size')
    with archive.open('rb') as stream:
        if hashlib.file_digest(stream, 'sha256').hexdigest() != data.get('archiveSha256'):
            raise ValueError('image_export_archive_hash')
    if component.startswith('runtime') and data.get('runtimeContract') != '3':
        raise ValueError('image_export_runtime_contract')
    return data, archive


def load_image(command, node, cluster, directory, component, revision, run_id, attempt):
    data, archive = checked_metadata(directory, component, revision, run_id, attempt)
    command(['docker', 'image', 'load', '--input', str(archive)], timeout=300)
    image = json.loads(command(['docker', 'image', 'inspect', data['imageId']]))[0]
    labels = image.get('Config', {}).get('Labels', {})
    if (image['Id'] != data['imageId'] or image['Architecture'] != 'amd64'
            or image['Os'] != 'linux' or labels.get('org.opencontainers.image.revision') != revision):
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
    if (image.get('Architecture') != 'amd64' or image.get('Os') != 'linux'
            or canonical_reference(pinned) not in {
                canonical_reference(ref) for ref in image.get('RepoDigests', [])}
            or not re.fullmatch('sha256:[0-9a-f]{64}', image.get('Id', ''))):
        raise ValueError('dependency_image_identity_mismatch')
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
    first = repository.split('/')[0]
    if '/' not in repository or ('.' not in first and ':' not in first and first != 'localhost'):
        repository = 'docker.io/' + repository
    if repository.startswith('docker.io/') and repository.count('/') == 1:
        repository = 'docker.io/library/' + repository.split('/', 1)[1]
    return repository + '@' + digest
