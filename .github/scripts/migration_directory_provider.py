#!/usr/bin/env python3
"""Finite synthetic identity profiles for the retained directory campaign."""
from copy import deepcopy
import hmac
from http.server import ThreadingHTTPServer
import json
from pathlib import Path

NAMES = {'old-write': 'Matrix 12d — retained Ω', 'new-write': 'Matrix 13b — retained 名',
         'reader': 'Provider replacement must not satisfy a retained directory read'}


def handler(identity):
    class DirectoryProvider(identity.Identity):
        profile_stage = 'reader'
        token_responses = 0

        def control_authorized(self):
            return hmac.compare_digest(self.headers.get('Authorization', ''),
                                       'Bearer ' + self.config['admin_bearer'])

        def summary(self):
            return {'profile_stage': type(self).profile_stage,
                    'token_responses': type(self).token_responses}

        def reply(self, status, body):
            if self.path == '/token' and status == 200:
                with self.lock:
                    type(self).token_responses += 1
            return super().reply(status, body)

        def do_POST(self):
            if not self.path.startswith('/fixture/directory/'):
                return super().do_POST()
            if not self.control_authorized():
                return self.reply(403, {'error': 'directory_control_denied'})
            stage = self.path.removeprefix('/fixture/directory/')
            if stage not in NAMES or self.headers.get('Content-Length', '0') != '0':
                return self.reply(400, {'error': 'directory_control_invalid'})
            with self.lock:
                type(self).profile_stage = stage
                summary = self.summary()
            return self.reply(200, summary)

        def do_GET(self):
            if self.path == '/fixture/directory/state':
                if not self.control_authorized():
                    return self.reply(403, {'error': 'directory_control_denied'})
                with self.lock:
                    summary = self.summary()
                return self.reply(200, summary)
            if self.path != '/userinfo':
                return super().do_GET()
            header = self.headers.get('Authorization', '')
            actor = self.actor(header[7:]) if header.startswith('Bearer ') else None
            if actor is None:
                return self.reply(401, {'error': 'unauthorized'})
            with self.lock:
                name = NAMES[type(self).profile_stage]
            return self.reply(200, {'sub': actor[0], 'name': name, 'permissions': actor[1]})
    return DirectoryProvider


def manifests(objects):
    objects = deepcopy(objects)
    selected = {(obj['kind'], obj['metadata']['name']): obj for obj in objects}
    config = selected[('ConfigMap', 'api-env')]['data']
    if config.get('CHRONO_AUTH_MODE') != 'provider':
        raise ValueError('directory fixture requires provider authentication')
    container = selected[('Deployment', 'identity')]['spec']['template']['spec']['containers'][0]
    if container['command'] != ['/usr/local/bin/python3', '-B', '/code/identity.py']:
        raise ValueError('directory identity command already customized')
    selected[('ConfigMap', 'identity-code')]['data']['migration_directory_provider.py'] = Path(__file__).read_text()
    container['command'][-1] = '/code/migration_directory_provider.py'
    return objects


if __name__ == '__main__':
    import identity
    provider = handler(identity)
    provider.config = json.loads(Path('/private/identity.json').read_text())
    server = ThreadingHTTPServer(('0.0.0.0', 8080), provider)
    server.daemon_threads = True
    server.serve_forever(poll_interval=0.2)
