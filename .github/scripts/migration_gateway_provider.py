#!/usr/bin/env python3
"""Opt-in gateway-record authority for the disposable identity dependency."""
import copy
from http.server import ThreadingHTTPServer
import json
from pathlib import Path


def configure(identity):
    # Keep ordinary users unprivileged so denial is exercised in every stage.
    identity.ADMIN_PERMISSIONS = list(identity.ADMIN_PERMISSIONS) + ['cma:gateway:records']
    return identity.Identity


def manifests(objects):
    objects = copy.deepcopy(objects)
    selected = {(obj['kind'], obj['metadata']['name']): obj for obj in objects}
    if selected[('ConfigMap', 'api-env')]['data'].get('CHRONO_AUTH_MODE') != 'provider':
        raise ValueError('gateway records require provider auth')
    container = selected[('Deployment', 'identity')]['spec']['template']['spec']['containers'][0]
    if container['command'] != ['/usr/local/bin/python3', '-B', '/code/identity.py']:
        raise ValueError('gateway identity command already customized')
    selected[('ConfigMap', 'identity-code')]['data']['migration_gateway_provider.py'] = (
        Path(__file__).read_text())
    container['command'][-1] = '/code/migration_gateway_provider.py'
    return objects


if __name__ == '__main__':
    import identity
    provider = configure(identity)
    provider.config = json.loads(Path('/private/identity.json').read_text())
    server = ThreadingHTTPServer(('0.0.0.0', 8080), provider)
    server.daemon_threads = True
    server.serve_forever(poll_interval=0.2)
