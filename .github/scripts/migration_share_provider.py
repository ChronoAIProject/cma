#!/usr/bin/env python3
"""One synthetic organization for the opt-in retained sharing campaign."""
import hmac
from http.server import ThreadingHTTPServer
import json
from pathlib import Path

ORGANIZATION = 'matrix-sharing-org'


def handler(identity):
    class ShareProvider(identity.Identity):
        def do_GET(self):
            if not self.path.startswith('/api/v1/orgs'):
                return super().do_GET()
            # Membership belongs only to the synthetic browser principal;
            # admin authority must not silently become organization membership.
            if not hmac.compare_digest(self.headers.get('Authorization', ''),
                                       'Bearer ' + self.config['bearer']):
                return self.reply(403, {'error': 'not_member'})
            row = {'id': ORGANIZATION, 'your_role': 'admin', 'display_name': 'Matrix sharing'}
            if self.path == '/api/v1/orgs':
                return self.reply(200, {'orgs': [row]})
            if self.path == '/api/v1/orgs/' + ORGANIZATION + '/authorization':
                return self.reply(200, row)
            return self.reply(404, {'error': 'unknown_organization'})
    return ShareProvider


def manifests(objects):
    from migration_sandbox_fixture import manifests as admission_manifests
    objects = admission_manifests(objects)
    selected = {(obj['kind'], obj['metadata']['name']): obj for obj in objects}
    container = selected[('Deployment', 'identity')]['spec']['template']['spec']['containers'][0]
    if container['command'] != ['/usr/local/bin/python3', '-B', '/code/identity.py']:
        raise ValueError('sharing identity command already customized')
    code = selected[('ConfigMap', 'identity-code')]['data']
    code['migration_share_provider.py'] = Path(__file__).read_text()
    container['command'][-1] = '/code/migration_share_provider.py'
    return objects


if __name__ == '__main__':
    import identity
    provider = handler(identity)
    provider.config = json.loads(Path('/private/identity.json').read_text())
    server = ThreadingHTTPServer(('0.0.0.0', 8080), provider)
    server.daemon_threads = True
    server.serve_forever(poll_interval=0.2)
