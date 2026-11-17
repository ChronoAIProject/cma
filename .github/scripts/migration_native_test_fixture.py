"""Ephemeral values for offline renderer tests; no runtime compatibility claim."""
import hashlib
import json
import secrets
from unittest.mock import patch

from migration_kind_manifests import manifests

CLUSTER = 'cma-matrix-0123456789'
NODE = CLUSTER + '-control-plane'


def runtime(config=None, manifest=None):
    source = 'a' * 40
    if config is None:
        config = {'os': 'linux', 'architecture': 'amd64', 'config': {
            'Labels': {'org.opencontainers.image.revision': source}}}
    config_bytes = json.dumps(config).encode()
    config_digest = 'sha256:' + hashlib.sha256(config_bytes).hexdigest()
    if manifest is None:
        manifest = {'schemaVersion': 2, 'config': {'digest': config_digest, 'size': len(config_bytes)}}
    manifest_bytes = json.dumps(manifest).encode()
    identity = {'source': source, 'config': config_digest,
                'image': 'registry.invalid/runtime@sha256:' + hashlib.sha256(manifest_bytes).hexdigest()}
    return identity, manifest_bytes, config_bytes


def fixture():
    credentials = {key: secrets.token_urlsafe(32) for key in (
        'client_id', 'client_secret', 'bearer', 'admin_bearer',
        'outsider_bearer', 'refresh_token', 'expires_at')}
    return manifests('registry.invalid/api@sha256:' + 'b' * 64,
                     secrets.token_hex(24), secrets.token_hex(24), secrets.token_hex(32),
                     'registry.invalid/mongo@sha256:' + 'c' * 64,
                     {'createRole': 'cma_am_runtime', 'privileges': [], 'roles': []},
                     identity_credentials=credentials)


def synthetic_tls():
    # Unit tests exercise hash binding with ephemeral bytes; they do not claim
    # actual certificate validity or change the committed public TLS pins.
    value = {key: secrets.token_urlsafe(48) for key in ('certificate', 'private_key')}
    pins = {key: hashlib.sha256(data.encode()).hexdigest() for key, data in value.items()}
    return value, patch('migration_native_endpoints.TLS_PINS', pins)


def arguments(tls):
    identity, manifest_bytes, config_bytes = runtime()
    return {'enabled': True, 'cluster': CLUSTER, 'node': NODE, 'runtime': identity,
            'runtime_manifest': manifest_bytes, 'runtime_config': config_bytes,
            'tls': tls, 'provider_control': secrets.token_urlsafe(32)}
