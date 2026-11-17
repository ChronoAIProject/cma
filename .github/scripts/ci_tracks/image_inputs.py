"""Public image recipes and the reviewed production model for broad COPYs.

Narrow COPYs and bind mounts always count, even when they contain tests/docs.
For Cargo's broad workspace COPY, production package files count except the
explicit integration-test/benchmark/example/doc trees and README files below.
Embedded files and build-script packages override those exclusions. For Vite,
only the named test trees/files are excluded, unless referenced by product code.
Unknown files are retained. No blanket extension exclusion is used.
"""

import hashlib
import json
from pathlib import Path
import re

from docker_inputs import ignored, matches, stage_sources
from policy import closure

MANIFEST = '.github/scripts/deployables.json'
NAMES = frozenset(json.loads((Path(__file__).resolve().parents[3] / MANIFEST).read_text()))
CARGO_CONFIG = {'api/Cargo.toml', 'api/Cargo.lock', 'api/rust-toolchain.toml'}


def recipes(root, fallback=None):
    path = root / MANIFEST
    data = json.loads(path.read_text()) if path.exists() else fallback
    if not isinstance(data, dict) or set(data) != NAMES:
        raise ValueError('incomplete image manifest')
    components = [recipe.get('component') for recipe in data.values() if isinstance(recipe, dict)]
    expected = {'api', 'cmaeg', 'web', 'runtime', 'runtime-desktop', 'runtime-omarchy', 'image-manager'}
    if len(data) != len(expected) or set(components) != expected:
        raise ValueError('ambiguous image components')
    for recipe in data.values():
        if set(recipe) != {'dockerfile', 'context', 'target', 'platforms', 'build_args', 'cargo_root', 'broad_copy', 'component', 'dockerfile_sha256', 'broad_copy_exclusions', 'workspace_exclusions'}:
            raise ValueError('invalid image recipe schema')
        if not isinstance(recipe['broad_copy_exclusions'], list) or any(not isinstance(p, str) for p in recipe['broad_copy_exclusions']):
            raise ValueError('invalid reviewed image exclusions')
        if recipe['context'] != '.' or recipe['broad_copy'] not in {'cargo-production', 'vite-production'}:
            raise ValueError('unsupported image build model')
    return data


def embedded_inputs(root, directories, tests):
    embedded = set()
    opaque = set()
    for directory in directories:
        if (root / directory / 'build.rs').exists():
            # Build scripts are executable input programs. Until their reads
            # are explicitly modeled, all package files can affect the image.
            opaque.add(directory)
        for file in (root / directory / 'src').rglob('*.rs'):
            if file.relative_to(root).as_posix() in tests:
                continue
            text = file.read_text()
            for match in re.finditer(r'include(?:_str|_bytes)?!\s*\(\s*"([^"]+)"', text):
                source = (file.parent / match[1]).resolve().relative_to(root.resolve()).as_posix()
                embedded.add(source)
            if re.search(r'include(?:_str|_bytes)?!\s*\(\s*[^"\s]', text):
                opaque.add(directory)
    return embedded, opaque


def cargo_inputs(root, workspace, package, files, exclusions, workspace_exclusions):
    packages = closure({package}, workspace.production)
    directories = {directory for directory, name in workspace.directories.items() if name in packages}
    from rust_inputs import test_modules
    tests = test_modules(root, directories)
    embedded, opaque = embedded_inputs(root, directories, tests)
    if opaque:
        # Arbitrary build scripts/dynamic includes may read outside their own
        # package. Keep the whole copied workspace until those reads are modeled.
        return set(files)
    selected = set()
    for path in files:
        if path in embedded or path in CARGO_CONFIG or path.startswith('api/.cargo/'):
            selected.add(path)
            continue
        directory = next((d for d in directories if path.startswith(d + '/')), None)
        if not directory:
            # New workspace-level build inputs are conservative. Other members
            # have an explicit production dependency boundary.
            if path.startswith('api/') and path not in workspace_exclusions and not any(path.startswith(d + '/') for d in workspace.directories):
                selected.add(path)
            continue
        relative = path[len(directory) + 1:]
        excluded = path in tests or any(matches(relative, pattern) for pattern in exclusions if pattern != 'cfg(test)-only modules')
        if not excluded or path in embedded or directory in opaque:
            selected.add(path)
    return selected


def image_inputs(root, workspace, manifest=None):
    root = Path(root)
    manifest = recipes(root, manifest)
    files = {p.relative_to(root).as_posix() for p in root.rglob('*') if p.is_file() and '.git' not in p.parts}
    result = {}
    for name, recipe in manifest.items():
        dockerfile = recipe['dockerfile']
        source = root / dockerfile
        if not source.is_file():
            raise ValueError('image Dockerfile is missing')
        if hashlib.sha256(source.read_bytes()).hexdigest() != recipe['dockerfile_sha256']:
            raise ValueError('Dockerfile changed; review and refresh its production-input model')
        ignore_path = dockerfile + '.dockerignore' if (root / (dockerfile + '.dockerignore')).exists() else '.dockerignore'
        ignore_text = (root / ignore_path).read_text() if (root / ignore_path).exists() else ''
        available = {p for p in files if not ignored(p, ignore_text)}
        sources = stage_sources(source.read_text(), recipe['target'], recipe['build_args'])
        inputs = {dockerfile, ignore_path}
        for pattern in sources:
            copied = {p for p in available if matches(p, pattern)}
            if not copied:
                raise ValueError('Docker source has no available inputs')
            if pattern == 'api' and recipe['broad_copy'] == 'cargo-production':
                copied = cargo_inputs(root, workspace, recipe['cargo_root'], copied, recipe['broad_copy_exclusions'], recipe['workspace_exclusions'])
            elif pattern == 'web' and recipe['broad_copy'] == 'vite-production':
                from web_inputs import web_inputs
                copied = web_inputs(root, copied, recipe['broad_copy_exclusions'])
            inputs.update(copied)
        result[name] = inputs
    return result


def changed_images(paths, snapshots):
    selected = set()
    for root, workspace, manifest in snapshots:
        for name, inputs in image_inputs(root, workspace, manifest).items():
            if set(paths) & inputs:
                selected.add(name)
    first, last = snapshots[0], snapshots[-1]
    before, after = recipes(first[0], first[2]), recipes(last[0], last[2])
    selected.update(name for name in NAMES if before[name] != after[name])
    # Workflow scheduling and runtime-generated matrices are not image inputs.
    # Build recipe changes belong in the per-deployable manifest above.
    return sorted(selected)
