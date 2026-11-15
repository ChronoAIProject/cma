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
    command(['kind', 'load', 'docker-image', '--name', cluster, tag], timeout=300)
    prefix = ['docker', 'exec', node, 'ctr', '-n', 'k8s.io']
    rows = command(prefix + ['images', 'list']).splitlines()
    matches = [row.split() for row in rows if row.split() and row.split()[0] == tag]
    if len(matches) != 1 or not re.fullmatch(r'sha256:[0-9a-f]{64}', matches[0][2]):
        raise ValueError('imported_manifest_missing')
    reference = tag.split(':')[0] + '@' + matches[0][2]
    command(prefix + ['images', 'tag', tag, reference])
    observed = json.loads(command(['docker', 'exec', node, 'crictl', 'inspecti', reference]))['status']
    if observed.get('id') != data['imageId']:
        raise ValueError('imported_config_mismatch')
    return reference
