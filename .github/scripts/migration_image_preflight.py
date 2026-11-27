"""Retain only the image fields used by compatibility checks, before kind starts."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import re
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


SOURCE = re.compile(r"^[a-f0-9]{40}$")
IMAGE = re.compile(r"^(?:[a-z0-9](?:[a-z0-9.-]*[a-z0-9])?(?::[0-9]+)?/)+"
                   r"[a-z0-9][a-z0-9._/-]*@sha256:[a-f0-9]{64}$")


class RunnerError(RuntimeError):
    pass

def checked_identity(command, image, source):
    if not SOURCE.fullmatch(source):
        raise RunnerError("source must be a full immutable SHA")
    if not IMAGE.fullmatch(image):
        raise RunnerError("image must be a full registry digest")
    try:
        rows = json.loads(command(["docker", "image", "inspect", image]))
    except (subprocess.CalledProcessError, json.JSONDecodeError) as error:
        raise RunnerError("image is not available locally") from error
    if len(rows) != 1:
        raise RunnerError("image inspect returned an unexpected count")
    row = rows[0]
    labels = row.get("Config", {}).get("Labels", {}) or {}
    if labels.get("org.opencontainers.image.revision") != source:
        raise RunnerError("image source label does not match selected source")
    if row.get("Architecture") != "amd64" or row.get("Os") != "linux":
        raise RunnerError("matrix images must be linux/amd64")
    digests = set(row.get("RepoDigests", []))
    if image not in digests and image.split("@", 1)[1] not in {d.split("@", 1)[-1] for d in digests}:
        raise RunnerError("image manifest digest is not present in local image metadata")
    config = row.get("Descriptor", {}).get("annotations", {}).get("config.digest")
    if config is None:
        config = manifest_config(command, image, row)
    if not isinstance(config, str) or not re.fullmatch(r"sha256:[a-f0-9]{64}", config):
        raise RunnerError("local image metadata lacks an explicit immutable config digest")
    return {"source": source, "image": image, "local_image_id": row.get("Id"), "config": config,
            "labels": {"revision": labels.get("org.opencontainers.image.revision"),
                       "source": labels.get("org.opencontainers.image.source")}}
MANIFESTS = frozenset({"application/vnd.oci.image.manifest.v1+json",
                       "application/vnd.docker.distribution.manifest.v2+json"})


def manifest_config(command, image, row):
    """Bind the config digest to the pinned manifest when Docker omits it.

    Docker's containerd store describes a pulled image by its manifest digest
    and carries no config annotation; the classic store has no descriptor and
    names the image by its config digest. Either way the pinned manifest's own
    bytes fix the config digest, so they are read and checked against the pin.
    """
    pinned = image.split("@", 1)[1]
    descriptor = row.get("Descriptor")
    if descriptor is not None and (descriptor.get("digest") != pinned
                                   or descriptor.get("mediaType") not in MANIFESTS):
        raise RunnerError("local image metadata lacks an explicit immutable config digest")
    try:
        raw = command(["docker", "buildx", "imagetools", "inspect", "--raw", image])
    except (subprocess.CalledProcessError, OSError, ValueError) as error:
        raise RunnerError("pinned image manifest is not readable") from error
    if "sha256:" + hashlib.sha256(raw.encode()).hexdigest() != pinned:
        raise RunnerError("pinned image manifest does not match its digest")
    try:
        manifest = json.loads(raw)
    except json.JSONDecodeError as error:
        raise RunnerError("pinned image manifest is not readable") from error
    # mediaType is optional in an OCI manifest; an index always lists manifests.
    if (not isinstance(manifest, dict) or "manifests" in manifest
            or manifest.get("mediaType", next(iter(sorted(MANIFESTS)))) not in MANIFESTS):
        raise RunnerError("pinned image must be a single-platform manifest")
    config = (manifest.get("config") or {}).get("digest")
    if descriptor is None and row.get("Id") != config:
        raise RunnerError("local image id does not match the pinned manifest config")
    return config


def checked_dependency(command, image):
    """Validate the disposable linux/amd64 Mongo image before kind load."""
    if not IMAGE.fullmatch(image):
        raise RunnerError("Mongo image must be a full registry digest")
    try:
        rows = json.loads(command(["docker", "image", "inspect", image]))
    except (subprocess.CalledProcessError, json.JSONDecodeError) as error:
        raise RunnerError("Mongo image is not available locally") from error
    if len(rows) != 1 or rows[0].get("Architecture") != "amd64" or rows[0].get("Os") != "linux":
        raise RunnerError("Mongo image must be linux/amd64")
    digests = set(rows[0].get("RepoDigests", []))
    if image not in digests and image.split("@", 1)[1] not in {d.split("@", 1)[-1] for d in digests}:
        raise RunnerError("Mongo image manifest digest is not present in local image metadata")
    return {"image": image, "local_image_id": rows[0].get("Id")}

def checked_identity_dependency(command, image):
    """Validate the reviewed synthetic provider runtime before kind load."""
    value = checked_dependency(command, image)
    if not image.startswith("docker.io/library/python@sha256:"):
        raise RunnerError("identity fixture must use the reviewed Python image")
    return value
