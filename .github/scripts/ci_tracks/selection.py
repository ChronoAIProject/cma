"""Shared PR/CD selector: test ownership and exact production image inputs."""

import argparse
from contextlib import contextmanager
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile

from image_inputs import NAMES, changed_input_maps, historical_recipe_model, image_inputs, recipes, validate_recipe_sources
from git_comparison import check_sha, comparison_details, git
from policy import TRACKS, Workspace, select
from publication_inputs import RECIPE
from uncertainty import SelectionIntegrityError, conservative, unknown_paths

APPROVAL_CONTRACT_INPUTS = frozenset({
    'web/vite.config.ts', 'web/tsconfig.json', 'web/package.json', 'web/package-lock.json',
    '.github/workflows/ci-approval-contract.yml', '.github/workflows/ci-rust-archive.yml',
    '.github/scripts/backend-nextest.sh',
})
APPROVAL_CONTRACT_DIRECTORIES = (
    'web/src/api-client/', 'web/src/api/', 'web/src/agent/', 'web/src/test/', 'web/build/',
    '.github/actions/rust-archive/',
)


def comparison(root, event, event_name):
    base, head, _reason = comparison_details(root, event, event_name)
    return base, head


def changed_paths(root, base, head):
    raw = git(root, 'diff', '--name-status', '-z', '--find-renames', check_sha(base), check_sha(head), '--')
    fields = raw.decode('utf-8').split('\0')
    paths = []
    while fields and fields[0]:
        status = fields.pop(0)
        if not re.fullmatch(r'[ACDMRTUXB][0-9]*', status):
            raise ValueError('invalid diff status')
        count = 2 if status[0] in 'RC' else 1
        if len(fields) <= count or any(not p for p in fields[:count]):
            raise ValueError('incomplete diff record')
        paths.extend(fields[:count])
        del fields[:count]
    if fields != ['']:
        raise ValueError('incomplete diff terminator')
    # Validate paths before any uncertainty fallback can produce a plan.
    select(paths, force_all=True)
    return sorted(set(paths))


@contextmanager
def snapshot(root, revision):
    with tempfile.TemporaryDirectory(prefix='cma-selector-') as directory:
        with subprocess.Popen(['git', 'archive', check_sha(revision)], cwd=root, stdout=subprocess.PIPE) as archive:
            subprocess.run(['tar', '-xf', '-', '-C', directory], stdin=archive.stdout, check=True, timeout=120)
            archive.stdout.close()
            if archive.wait(timeout=120) != 0:
                raise ValueError('cannot read comparison tree')
        yield Path(directory)


def workspace(root):
    command = ['cargo', 'metadata', '--format-version', '1', '--locked', '--no-deps']
    run = subprocess.run(command, cwd=root / 'api', check=True, capture_output=True, timeout=120)
    return Workspace(json.loads(run.stdout), root)


def summarize(root, fallback, *, historical=False):
    graph = workspace(root)
    return summarize_graph(root, graph, fallback, historical=historical)


def summarize_graph(root, graph, fallback, *, historical=False):
    manifest = recipes(root, fallback)
    validate_recipe_sources(root, historical_recipe_model(root, manifest) if historical else manifest)
    try:
        inputs = image_inputs(root, graph, manifest, historical=historical)
    except SelectionIntegrityError:
        raise
    except (ValueError, KeyError, OSError):
        # Only production-input modeling is recoverable. Workspace ownership,
        # source identity, reviewed recipe hashes and cache authority stay strict.
        inputs = None
    return graph, inputs, manifest


def analyze_summaries(paths, summaries, *, read_runtime_recipes=None):
    select(paths, force_all=True)
    if any(inputs is None for _, inputs, _ in summaries):
        return conservative(NAMES, 'input-analysis-uncertain', summaries[-1][0], len(paths))
    if unknown_paths(paths, [graph for graph, _, _ in summaries]):
        return conservative(NAMES, 'unknown-input-path', summaries[-1][0], len(paths))
    results = [select(paths, graph) for graph, _, _ in summaries]
    result = results[-1]
    for track in ('am', 'eg', 'web'):
        result[track] = any(item[track] for item in results)
    # Package suites must come from the current tree; removed packages can
    # select a track through the old graph but cannot appear in its test argv.
    for track in ('am', 'eg'):
        if result[track] and not result[track + '_packages']:
            result[track + '_packages'] = sorted(summaries[-1][0].owned[track])
    result['test_tracks'] = {key: result[key] for key in ('repo', 'am', 'eg', 'web')}
    result['deployables'] = changed_input_maps(
        paths, [(inputs, manifest) for _, inputs, manifest in summaries],
        read_runtime_recipes=read_runtime_recipes)
    result['changed_path_count'] = len(paths)
    return result


def analyze(paths, snapshots):
    summaries = [summarize_graph(tree, graph, fallback) for tree, graph, fallback in snapshots]
    return analyze_summaries(paths, summaries)


def analyze_comparison(root, base, head, summaries):
    paths = changed_paths(root, base, head)

    def read_runtime_recipes():
        return tuple(git(root, 'show', f'{revision}:{RECIPE}').decode('utf-8') for revision in (base, head))

    return analyze_summaries(paths, summaries, read_runtime_recipes=read_runtime_recipes)


def determine(root, event, event_name, *, release=False):
    base, head, reason = comparison_details(root, event, event_name)
    fallback = recipes(root)
    with snapshot(root, head) as new:
        current = summarize(new, fallback)
    previous = None
    if not reason:
        try:
            with snapshot(root, base) as old:
                previous = summarize(old, fallback, historical=True)
        except (ValueError, KeyError, OSError, subprocess.SubprocessError):
            # A develop-to-main release compares against the previous release,
            # which can predate the current ownership policy and recipes. That
            # base cannot narrow a full-CI release, so failing to model it is
            # uncertainty and selects every deployable. The head stays strict,
            # and any other comparison still fails closed.
            if not release:
                raise
            reason = 'release-base-unmodeled'
    if reason:
        result = conservative(NAMES, reason, current[0])
    else:
        result = analyze_comparison(root, base, head, [previous, current])
    result.update(base=base, head=head)
    return result


def validate_ci_checkout(root, event, event_name):
    checkout = check_sha(os.environ['GITHUB_SHA'])
    if checkout == '0' * 40 or git(root, 'rev-parse', 'HEAD').decode().strip() != checkout:
        raise SelectionIntegrityError('CI checkout differs from event SHA')
    if event_name == 'merge_group' and checkout != event['merge_group']['head_sha']:
        raise SelectionIntegrityError('merge queue checkout differs from event head')


def apply_pr_rust_coverage(root, paths, result, full):
    graph = workspace(root)
    # Removed paths retain their original tracks. Only current paths can add owners.
    current_paths = [path for path in paths if not unknown_paths([path], [graph])]
    coverage = select(current_paths, graph, force_all=full or all(result[track] for track in TRACKS))
    for track in ('am', 'eg'):
        if coverage[track] or result[track]:
            result[track] = True
            result[track + '_packages'] = coverage[track + '_packages'] or sorted(graph.owned[track])


def determine_ci(root, event, event_name, repository):
    from ci_events import VERIFICATION_EVENTS, full_ci_required, verification_head

    full = full_ci_required(event, event_name, repository)
    if event_name in VERIFICATION_EVENTS:
        result = determine(root, {'after': verification_head()}, 'push')
    else:
        result = determine(root, event, event_name, release=full and event_name == 'pull_request')
    paths = [] if full else changed_paths(root, result['base'], result['head'])
    if event_name == 'pull_request' and (full or result['am'] or result['eg']):
        apply_pr_rust_coverage(root, paths, result, full)
    elif full:
        with snapshot(root, result['head']) as tree:
            coverage = select([], workspace(tree), force_all=True)
        for key in ('am_packages', 'eg_packages'):
            result[key] = coverage[key]
    if full:
        for track in TRACKS:
            result[track] = True
        result['reasons'] = sorted(set(result['reasons']) | {'full-ci-event'})
    result['test_tracks'] = {key: result[key] for key in ('repo', 'am', 'eg', 'web')}
    # A Web consumer change can require the shared Rust test archive without
    # changing Agent Manager's product test or image scope.
    result['approval_contract'] = result['am'] or any(
        path in APPROVAL_CONTRACT_INPUTS or path.startswith(APPROVAL_CONTRACT_DIRECTORIES)
        for path in paths
    )
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--root', type=Path, default=Path.cwd())
    parser.add_argument('--event', type=Path, default=os.environ.get('GITHUB_EVENT_PATH'))
    parser.add_argument('--event-name', default=os.environ.get('GITHUB_EVENT_NAME'))
    parser.add_argument('--ci', action='store_true', help='enforce CI event and full coverage policy')
    args = parser.parse_args()
    try:
        event = json.loads(args.event.read_text()) if args.event else {}
        if args.ci:
            root = args.root.resolve()
            validate_ci_checkout(root, event, args.event_name)
            result = determine_ci(root, event, args.event_name, os.environ['GITHUB_REPOSITORY'])
        else:
            result = determine(args.root.resolve(), event, args.event_name)
    except (KeyError, TypeError, ValueError, subprocess.SubprocessError, OSError):
        # An image-analysis failure must never become a successful empty list.
        # Suppress subprocess output, which may contain arbitrary PR content.
        print('::error::Cannot establish changed image inputs; selector failed closed.')
        return 1
    print(json.dumps(result, sort_keys=True))
    if os.environ.get('GITHUB_OUTPUT'):
        with open(os.environ['GITHUB_OUTPUT'], 'a', encoding='utf-8') as output:
            keys = ('repo', 'am', 'eg', 'web', 'am_packages', 'eg_packages', 'test_tracks',
                    'deployables', 'base', 'head', 'reasons')
            if args.ci:
                keys += ('approval_contract',)
            for key in keys:
                value = result[key] if isinstance(result[key], str) else json.dumps(result[key], separators=(',', ':'))
                output.write(f"{key}={value}\n")
    return 0


if __name__ == '__main__':
    sys.exit(main())
