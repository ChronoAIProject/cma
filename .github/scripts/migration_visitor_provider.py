"""Synthetic visitor authority and explicitly selected Launch admission policy."""
import json
from http.server import ThreadingHTTPServer
from pathlib import Path
import secrets

from migration_launch_provider import handler as launch_handler


def handler(identity):
    class VisitorProvider(launch_handler(identity)):
        def actor(self, token):
            actor = super().actor(token)
            if actor is not None and actor[0] == identity.OWNER:
                return actor[0], [p for p in actor[1] if p != 'cma:sandbox:create']
            return actor
    return VisitorProvider


def manifests(objects):
    from migration_sandbox_fixture import manifests as without_runtime
    from migration_launch_documents import infrastructure
    objects = without_runtime(objects)
    selected = {(obj['kind'], obj['metadata']['name']): obj for obj in objects}
    api_env = selected[('ConfigMap', 'api-env')]['data']
    flag = 'CHRONO_TRIGGER_COMMAND_ADMISSION_ENABLED'
    if flag in api_env:
        raise ValueError('Launch admission policy already selected')
    api_env[flag] = 'true'
    provider = selected[('Deployment', 'identity')]['spec']['template']['spec']['containers'][0]
    if provider['command'] != ['/usr/local/bin/python3', '-B', '/code/identity.py']:
        raise ValueError('visitor identity command already customized')
    code = selected[('ConfigMap', 'identity-code')]['data']
    code['migration_launch_provider.py'] = Path(__file__).with_name('migration_launch_provider.py').read_text()
    code['migration_visitor_provider.py'] = Path(__file__).read_text()
    provider['command'][-1] = '/code/migration_visitor_provider.py'
    namespace = selected[('Deployment', 'api')]['metadata']['namespace']
    return infrastructure(objects, namespace, secrets.token_urlsafe(24))


if __name__ == '__main__':
    import identity
    provider = handler(identity)
    provider.config = json.loads(Path('/private/identity.json').read_text())
    server = ThreadingHTTPServer(('0.0.0.0', 8080), provider)
    server.daemon_threads = True
    server.serve_forever(poll_interval=0.2)
