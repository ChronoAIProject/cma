"""Each develop PR and its merge-queue entry require one version label, whatever the selection."""

import json
import os
from pathlib import Path
import sys
import subprocess

from ci_events import full_ci_required, queued_pull_number, repository_name
from git_comparison import check_sha
from image_inputs import NAMES

LABELS = frozenset({'version-upgrade:major', 'version-upgrade:minor', 'version-upgrade:patch'})


def deployables(value):
    if not isinstance(value, list) or any(not isinstance(item, str) or item not in NAMES for item in value):
        raise ValueError('invalid deployable selection')
    if value != sorted(set(value)):
        raise ValueError('deployable selection must be sorted and unique')
    return value


def selected_upgrade(event, images):
    deployables(images)
    labels = event['pull_request']['labels']
    if not isinstance(labels, list) or any(not isinstance(item, dict) or not isinstance(item.get('name'), str) for item in labels):
        raise ValueError('malformed PR labels')
    # Match allocation: unknown labels in this namespace cannot hide beside a valid one.
    selected = [item['name'] for item in labels if item['name'].startswith('version-upgrade:')]
    if len(selected) != 1 or selected[0] not in LABELS:
        raise ValueError('exactly one version-upgrade:major, version-upgrade:minor or version-upgrade:patch label is required')
    return selected[0].split(':')[1]


def github(path):
    result = subprocess.run(['gh', 'api', path, '-H', 'Accept: application/vnd.github+json'],
                            check=True, capture_output=True, text=True, timeout=30)
    return json.loads(result.stdout)


def current_pull(event, repository):
    repository_name(repository)
    expected = event['pull_request']
    number = event['number']
    if type(number) is not int or number <= 0 or event['repository']['full_name'] != repository:
        raise ValueError('event repository/PR identity differs')
    current = github(f'repos/{repository}/pulls/{number}')
    if current['number'] != number or current['state'] != 'open':
        raise ValueError('PR is no longer open at the checked identity')
    for side in ('base', 'head'):
        for key in ('ref', 'sha'):
            if current[side][key] != expected[side][key]:
                raise ValueError('PR base/head changed since this check event')
        if current[side]['repo']['full_name'] != expected[side]['repo']['full_name']:
            raise ValueError('PR source repository changed')
    full_ci_required(dict(event, pull_request=current), 'pull_request', repository)
    return current


def queued_pull(event, repository):
    """Read the live labels of the develop PR this merge-queue entry tests."""
    repository_name(repository)
    number = queued_pull_number(event)
    if event['repository']['full_name'] != repository:
        raise ValueError('event repository/PR identity differs')
    current = github(f'repos/{repository}/pulls/{number}')
    if current['number'] != number or current['state'] != 'open':
        raise ValueError('queued PR is no longer open at the checked identity')
    if current['base']['ref'] != 'develop' or current['base']['repo']['full_name'] != repository:
        raise ValueError('queued PR does not target this develop branch')
    head = check_sha(current['head']['sha'])
    group = check_sha(event['merge_group']['head_sha'])
    # With merge commits the queued head is an ancestor of the group commit. A
    # head pushed after queueing, or another merge method, cannot borrow a label.
    comparison = github(f'repos/{repository}/compare/{head}...{group}')
    if comparison['status'] not in ('ahead', 'identical') or comparison['behind_by'] != 0:
        raise ValueError('queued PR head is not contained in the merge group (merge method must be MERGE)')
    return current


def main():
    try:
        if os.environ['SELECTOR_RESULT'] != 'success':
            raise ValueError('successful CI selection required')
        event = json.loads(Path(os.environ['GITHUB_EVENT_PATH']).read_text())
        images = deployables(json.loads(os.environ['DEPLOYABLES']))
        print('Deployables: ' + (', '.join(images) or '(none)'))
        event_name = os.environ['GITHUB_EVENT_NAME']
        full = full_ci_required(event, event_name, os.environ['GITHUB_REPOSITORY'])
        if event_name == 'pull_request':
            current = current_pull(event, os.environ['GITHUB_REPOSITORY'])
        elif event_name == 'merge_group':
            current = queued_pull(event, os.environ['GITHUB_REPOSITORY'])
            print(f"Queued PR: #{current['number']}")
        if full:
            print('Version policy: not applicable for full CI')
            return 0
        upgrade = selected_upgrade({'pull_request': current}, images)
    except (KeyError, OSError, TypeError, ValueError, subprocess.SubprocessError) as error:
        print(f'::error::Version label check failed: {error}')
        return 1
    print(f'Version policy: {upgrade}')
    return 0


if __name__ == '__main__':
    sys.exit(main())
