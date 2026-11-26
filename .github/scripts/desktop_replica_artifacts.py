"""Bind desktop execution to a clean source identity and private binary copies."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess

from desktop_replica_results import json_value, require


def digest(path):
    value = hashlib.sha256()
    with Path(path).open('rb') as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b''):
            value.update(chunk)
    return value.hexdigest()


def command(root, args, env=None):
    return subprocess.check_output(args, cwd=root, env=env, text=True, stderr=subprocess.PIPE).strip()


def source_identity(root):
    require(not command(root, ['git', 'status', '--porcelain=v1', '--untracked-files=normal']),
            'Desktop qualification requires a clean candidate source tree')
    return {'commit': command(root, ['git', 'rev-parse', 'HEAD']),
            'tree': command(root, ['git', 'rev-parse', 'HEAD^{tree}'])}


def toolchain(root, env):
    return {name: command(root / 'api', args, env) for name, args in {
        'rustc': [env.get('RUSTC', 'rustc'), '-Vv'], 'cargo': ['cargo', '--version'],
        'nextest': ['cargo', 'nextest', '--version'],
    }.items()}


def settings(root, env):
    names = ['Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', '.cargo/config.toml']
    files = {name: digest(root / 'api' / name) for name in names if (root / 'api' / name).is_file()}
    variables = {key: value for key, value in env.items() if key in {
        'RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'CARGO_BUILD_TARGET', 'CARGO_INCREMENTAL',
        'RUSTC', 'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER',
    } or key.startswith('CARGO_PROFILE_')}
    return hashlib.sha256(json.dumps({'files': files, 'environment': variables}, sort_keys=True).encode()).hexdigest()


def freeze(directory, inventory, binaries):
    destination = directory / 'binaries'
    destination.mkdir()
    copied = {}
    hashes = {}
    for name, original in binaries.items():
        expected = digest(original)
        path = destination / name
        shutil.copy2(original, path)
        require(digest(path) == expected and digest(original) == expected, 'Binary changed while being captured')
        copied[name] = path
        hashes[name] = expected
    suites = {}
    keys = ('binary-id', 'binary-name', 'package-id', 'kind', 'binary-path', 'build-platform')
    for suite in inventory['rust-suites'].values():
        for name, original in binaries.items():
            if Path(suite['binary-path']) == original:
                row = {key: suite[key] for key in keys}
                row['binary-path'] = str(copied[name])
                suites[row['binary-id']] = row
    require(len(suites) == 2, 'Expected exactly two frozen test binaries')
    (directory / 'binaries-metadata.json').write_text(json.dumps({
        'rust-build-meta': inventory['rust-build-meta'], 'rust-binaries': suites,
    }, indent=2) + '\n')
    return copied, hashes


def identity(root, env, profile):
    return {'schema': 1, 'source': source_identity(root), 'toolchain': toolchain(root, env),
            'cargo_profile': profile, 'build_settings_sha256': settings(root, env)}


def archive_manifest(path, archive, current, hashes):
    require(path is not None and archive is not None, 'Archive reuse requires --artifact-manifest and --archive')
    manifest = json_value(path.read_text())
    require(isinstance(manifest, dict), "Archive identity must be an object")
    require(type(manifest.get("schema")) is int, "Archive identity mismatch: schema")
    for field, expected in {**current, 'binaries': hashes, 'archive_sha256': digest(archive)}.items():
        require(manifest.get(field) == expected, f'Archive identity mismatch: {field}')
    return manifest
