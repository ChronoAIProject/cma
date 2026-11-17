"""A version label is required only for changed deployable image inputs."""

import json
import os
from pathlib import Path
import sys

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
    selected = [item['name'] for item in labels if item['name'] in LABELS]
    if not images:
        return 'ignored' if selected else 'not-required'
    if len(selected) != 1:
        raise ValueError('exactly one version-upgrade:major, version-upgrade:minor or version-upgrade:patch label is required')
    return selected[0].split(':')[1]


def main():
    try:
        if os.environ['GITHUB_EVENT_NAME'] != 'pull_request' or os.environ['SELECTOR_RESULT'] != 'success':
            raise ValueError('successful PR selection required')
        event = json.loads(Path(os.environ['GITHUB_EVENT_PATH']).read_text())
        images = deployables(json.loads(os.environ['DEPLOYABLES']))
        print('Deployables: ' + (', '.join(images) or '(none)'))
        upgrade = selected_upgrade(event, images)
    except (KeyError, OSError, TypeError, ValueError) as error:
        print(f'::error::Version label check failed: {error}')
        return 1
    if upgrade == 'ignored':
        print('::notice::No deployable image inputs changed; version labels are ignored.')
    print(f'Version policy: {upgrade}')
    return 0


if __name__ == '__main__':
    sys.exit(main())
