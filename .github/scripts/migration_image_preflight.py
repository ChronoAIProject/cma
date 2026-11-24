"""Retain only the image fields used by compatibility checks, before kind starts."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import time


def identity_metadata(text):
    rows = json.loads(text)
    if not isinstance(rows, list) or any(not isinstance(row, dict) for row in rows):
        raise ValueError('image inspection must contain an array of objects')
    result = []
    for row in rows:
        # Images may contain arbitrary build labels and environment variables.
        # Preserve the exact identity fields without copying those other values.
        retained = {key: row[key] for key in ('Id', 'Architecture', 'Os', 'RepoDigests')
                    if key in row}
        config = row.get('Config') or {}
        labels = config.get('Labels') or {}
        descriptor = row.get('Descriptor') or {}
        annotations = descriptor.get('annotations') or {}
        if not all(isinstance(value, dict) for value in (config, labels, descriptor, annotations)):
            raise ValueError('image metadata objects are malformed')
        retained['Config'] = {'Labels': {key: labels[key] for key in (
            'org.opencontainers.image.revision', 'org.opencontainers.image.source') if key in labels}}
        # The classic store has no descriptor; its absence selects how the
        # config digest is bound, so it must not be replaced by an empty one.
        if 'Descriptor' in row:
            kept = {key: descriptor[key] for key in ('mediaType', 'digest') if key in descriptor}
            kept['annotations'] = {key: annotations[key] for key in ('config.digest',) if key in annotations}
            retained['Descriptor'] = kept
        result.append(retained)
    return result


def manifest_metadata(text):
    document = json.loads(text)
    if not isinstance(document, dict) or not isinstance(document.get('config') or {}, dict):
        raise ValueError('image manifest must be an object')
    return {'manifest_sha256': hashlib.sha256(text.encode()).hexdigest(),
            'mediaType': document.get('mediaType'),
            'config': (document.get('config') or {}).get('digest')}


class ImagePreflight:
    def __init__(self, output, command):
        self.path = Path(output) / 'image-preflight.private.json'
        self.command = command
        self.observations = []

    def __call__(self, argv):
        inspection = len(argv) == 4 and argv[:3] == ['docker', 'image', 'inspect']
        manifest = len(argv) == 6 and argv[:5] == ['docker', 'buildx', 'imagetools', 'inspect', '--raw']
        if not (inspection or manifest):
            raise ValueError('image preflight only permits Docker image inspection')
        row = {'argv': argv, 'started_at': time.time()}
        try:
            value = self.command(argv)
            row['exit'] = 0
            if manifest:
                # The raw manifest is returned unchanged for its digest check;
                # only its identity, never its layer list, is retained.
                row['metadata'] = manifest_metadata(value)
                return value
            row['metadata'] = identity_metadata(value)
            return json.dumps(row['metadata'])
        except (ValueError, AttributeError, TypeError) as error:
            row['error'] = 'invalid_image_metadata'
            raise ValueError('invalid image inspection metadata') from error
        except (subprocess.SubprocessError, OSError, KeyboardInterrupt, SystemExit) as error:
            row['error'] = type(error).__name__
            row['exit'] = error.returncode if isinstance(error, subprocess.CalledProcessError) else None
            raise
        finally:
            row['finished_at'] = time.time()
            self.observations.append(row)
            # A later failed image check must leave earlier observations intact.
            temporary = self.path.with_suffix('.tmp')
            with temporary.open('x', encoding='utf-8') as stream:
                os.chmod(temporary, 0o600)
                json.dump({'schema': 'migration-image-preflight-v1',
                           'observations': self.observations}, stream, indent=2)
                stream.write('\n')
            temporary.replace(self.path)
