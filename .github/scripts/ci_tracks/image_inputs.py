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

from deployable_manifest import CURRENT, validate_names
from docker_inputs import ignored, matches, stage_sources
from policy import closure
from publication_inputs import contract_images
from rust_build_inputs import inputs as reviewed_build_inputs
from uncertainty import SelectionIntegrityError

MANIFEST = '.github/scripts/deployables.json'
NAMES = frozenset(CURRENT)
CARGO_CONFIG = {'api/Cargo.toml', 'api/Cargo.lock', 'api/rust-toolchain.toml'}


def recipes(root, fallback=None):
    path = root / MANIFEST
    data = json.loads(path.read_text()) if path.exists() else fallback
    try:
        validate_names(data, historical=True)
    except ValueError as error:
        raise SelectionIntegrityError(str(error)) from error
    for recipe in data.values():
        if set(recipe) != {'dockerfile', 'context', 'target', 'platforms', 'build_args', 'cargo_root', 'broad_copy', 'component', 'dockerfile_sha256', 'broad_copy_exclusions', 'workspace_exclusions'}:
            raise SelectionIntegrityError('invalid image recipe schema')
        if not isinstance(recipe['broad_copy_exclusions'], list) or any(not isinstance(p, str) for p in recipe['broad_copy_exclusions']):
            raise SelectionIntegrityError('invalid reviewed image exclusions')
        if recipe['context'] != '.' or recipe['broad_copy'] not in {'cargo-production', 'vite-production'}:
            raise SelectionIntegrityError('unsupported image build model')
    return data


def embedded_inputs(root, directories, tests, build_scripts):
    # Manifest-only consumers do not install the Rust syntax parser.
    from rust_inputs import included_files

    embedded = set()
    opaque = set()
    root = root.resolve()
    for directory in directories:
        targets = build_scripts[directory].copy()
        if (root / directory / 'build.rs').exists():
            targets.add(f'{directory}/build.rs')
        reviewed_consumers = set()
        if targets:
            reviewed = reviewed_build_inputs(root, directory, targets)
            if reviewed is None:
                # A build program can read the whole workspace, including
                # files outside its package or Cargo's dependency closure.
                opaque.add(directory)
            else:
                required, reviewed_consumers = reviewed
                embedded.update(required)
        source_root = root / directory / 'src'
        for file in source_root.rglob('*.rs'):
            relative = file.relative_to(root).as_posix()
            if relative in tests or relative in reviewed_consumers:
                continue
            try:
                for macro, scope, path in included_files(file.read_bytes()):
                    base = root / directory if scope == 'manifest' else file.parent
                    target = (base / path).resolve()
                    name = target.relative_to(root).as_posix()
                    # An item include expands Rust code. Account for it only
                    # when the same package's source scan also reads the file.
                    if macro == b'include' and (not target.is_relative_to(source_root) or target.suffix != '.rs'
                                                or name in tests or not target.is_file()):
                        raise ValueError('item include is outside the scanned package sources')
                    embedded.add(name)
            except ValueError:
                opaque.add(directory)
    return embedded, opaque


def cargo_inputs(root, workspace, package, files, exclusions, workspace_exclusions, analysis=None, source_cache=None):
    from rust_inputs import test_modules

    packages = closure({package}, workspace.production)
    directories = {directory for directory, name in workspace.directories.items() if name in packages}
    # Runtime variants share source analysis. This cache belongs to one
    # immutable snapshot invocation; a later call must reread changed files.
    analysis = {} if analysis is None else analysis
    key = frozenset(directories)
    if key not in analysis:
        tests = test_modules(root, directories, source_cache)
        embedded, opaque = embedded_inputs(root, directories, tests, workspace.build_scripts)
        analysis[key] = tests, embedded, opaque
    tests, embedded, opaque = analysis[key]
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


def validate_recipe_sources(root, manifest):
    for name, recipe in manifest.items():
        if name not in NAMES:
            continue
        source = root / recipe['dockerfile']
        if not source.is_file():
            raise SelectionIntegrityError('image Dockerfile is missing')
        if hashlib.sha256(source.read_bytes()).hexdigest() != recipe['dockerfile_sha256']:
            raise SelectionIntegrityError('Dockerfile changed; review and refresh its production-input model')


def image_inputs(root, workspace, manifest=None):
    root = Path(root)
    manifest = recipes(root, manifest)
    validate_recipe_sources(root, manifest)
    files = {p.relative_to(root).as_posix() for p in root.rglob('*') if p.is_file() and '.git' not in p.parts}
    result, contexts, analysis, source_cache = {}, {}, {}, {}
    for name, recipe in manifest.items():
        if name not in NAMES:
            continue
        dockerfile = recipe['dockerfile']
        source = root / dockerfile
        ignore_path = dockerfile + '.dockerignore' if (root / (dockerfile + '.dockerignore')).exists() else '.dockerignore'
        ignore_text = (root / ignore_path).read_text() if (root / ignore_path).exists() else ''
        if ignore_text not in contexts:
            contexts[ignore_text] = {p for p in files if not ignored(p, ignore_text)}
        available = contexts[ignore_text]
        sources = stage_sources(source.read_text(), recipe['target'], recipe['build_args'])
        inputs = {dockerfile, ignore_path}
        for pattern in sources:
            copied = {p for p in available if matches(p, pattern)}
            if not copied:
                raise ValueError('Docker source has no available inputs')
            if pattern == 'api' and recipe['broad_copy'] == 'cargo-production':
                copied = cargo_inputs(root, workspace, recipe['cargo_root'], copied, recipe['broad_copy_exclusions'], recipe['workspace_exclusions'], analysis, source_cache)
            elif pattern == 'web' and recipe['broad_copy'] == 'vite-production':
                from web_inputs import web_inputs
                copied = web_inputs(root, copied, recipe['broad_copy_exclusions'])
            inputs.update(copied)
        result[name] = inputs
    return result


def changed_images(paths, snapshots):
    summaries = [(image_inputs(root, workspace, manifest), recipes(root, manifest))
                 for root, workspace, manifest in snapshots]
    return changed_input_maps(paths, summaries)


def changed_input_maps(paths, summaries, *, read_runtime_recipes=None):
    changed = set(paths)
    selected = {name for inputs, _ in summaries for name, files in inputs.items() if changed & files}
    before, after = summaries[0][1], summaries[-1][1]
    selected.update(name for name in NAMES if before[name] != after[name])
    # A lane is selected only by its own inputs. A publisher repair no longer
    # rebuilds current lanes; allocation retries any lane left behind.
    selected.update(contract_images(changed, read_runtime_recipes))
    return sorted(selected)
