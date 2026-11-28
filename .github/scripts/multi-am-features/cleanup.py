#!/usr/bin/env python3
"""Cancellation fallback: remove only the fixture's recorded Kind cluster."""
import json
from pathlib import Path
import re
import sys

from infrastructure import Infrastructure


def cleanup(directory):
    directory = Path(directory).resolve()
    state_path = directory / 'cluster-state.private.json'
    if not state_path.exists():
        return
    if state_path.is_symlink():
        raise ValueError('cluster_state_symlink')
    state = json.loads(state_path.read_text())
    if not re.fullmatch(r'cma-ci-[0-9a-f]{12}', state.get('cluster', '')):
        raise ValueError('cluster_state_identity')
    if state.get('node') != state['cluster'] + '-control-plane':
        raise ValueError('cluster_state_node')
    if not isinstance(state.get('previous_volumes'), list) or not isinstance(state.get('volumes'), list):
        raise ValueError('cluster_state_volume_identity')
    infra = Infrastructure(directory)
    for name in ['cluster', 'node', 'created', 'node_id', 'volumes', 'previous_volumes']:
        setattr(infra, name, state[name])
    infra.cleanup()


if __name__ == '__main__':
    try:
        cleanup(sys.argv[1])
    except Exception as error:
        print('owned_cluster_cleanup_failed:' + type(error).__name__, file=sys.stderr)
        raise SystemExit(1)
