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

from image_inputs import changed_input_maps, image_inputs, recipes
from policy import Workspace, select


def git(root, *args):
    return subprocess.run(['git', *args], cwd=root, check=True,
                          capture_output=True, timeout=60).stdout


def check_sha(ref):
    if not isinstance(ref, str) or not re.fullmatch(r'[0-9a-f]{40}', ref):
        raise ValueError('invalid comparison SHA')
    return ref


def comparison(root, event, event_name):
    if event_name == 'pull_request':
        pull = event['pull_request']
        head = check_sha(pull['head']['sha'])
        base = git(root, 'merge-base', check_sha(pull['base']['sha']), head).decode().strip()
    elif event_name == 'push':
        # CD runs this once for each merged PR, so compare the merge result to
        # its first parent, not an arbitrary push.before covering several PRs.
        head = check_sha(event['after'])
        base = git(root, 'rev-parse', head + '^1').decode().strip()
    else:
        raise ValueError('unsupported selector event')
    return check_sha(base), head


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


def summarize(root, fallback):
    graph = workspace(root)
    return graph, image_inputs(root, graph, fallback), recipes(root, fallback)


def analyze_summaries(paths, summaries):
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
    result['deployables'] = changed_input_maps(paths, [(inputs, manifest) for _, inputs, manifest in summaries])
    result['changed_path_count'] = len(paths)
    return result


def analyze(paths, snapshots):
    summaries = [(graph, image_inputs(tree, graph, fallback), recipes(tree, fallback))
                 for tree, graph, fallback in snapshots]
    return analyze_summaries(paths, summaries)


def determine(root, event, event_name):
    base, head = comparison(root, event, event_name)
    fallback = recipes(root)
    with snapshot(root, base) as old, snapshot(root, head) as new:
        snapshots = [(tree, workspace(tree), fallback) for tree in (old, new)]
        result = analyze(changed_paths(root, base, head), snapshots)
    result.update(base=base, head=head)
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--root', type=Path, default=Path.cwd())
    parser.add_argument('--event', type=Path, default=os.environ.get('GITHUB_EVENT_PATH'))
    parser.add_argument('--event-name', default=os.environ.get('GITHUB_EVENT_NAME'))
    args = parser.parse_args()
    try:
        event = json.loads(args.event.read_text()) if args.event else {}
        result = determine(args.root.resolve(), event, args.event_name)
    except (KeyError, TypeError, ValueError, subprocess.SubprocessError, OSError):
        # An image-analysis failure must never become a successful empty list.
        # Suppress subprocess output, which may contain arbitrary PR content.
        print('::error::Cannot establish changed image inputs; selector failed closed.')
        return 1
    print(json.dumps(result, sort_keys=True))
    if os.environ.get('GITHUB_OUTPUT'):
        with open(os.environ['GITHUB_OUTPUT'], 'a', encoding='utf-8') as output:
            for key in ('repo', 'am', 'eg', 'web', 'am_packages', 'eg_packages', 'test_tracks', 'deployables', 'base', 'head'):
                value = result[key] if isinstance(result[key], str) else json.dumps(result[key], separators=(',', ':'))
                output.write(f"{key}={value}\n")
    return 0


if __name__ == '__main__':
    sys.exit(main())
