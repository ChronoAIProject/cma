"""Bounded synthetic NyxID connect links; no runtime or assignment emulation."""
from copy import deepcopy
from datetime import datetime, timedelta, timezone
import hmac
from http.server import ThreadingHTTPServer
import json
from pathlib import Path
import re
import secrets

SERVICE = 'matrix-connect'
STATES = ('pending', 'completed', 'expired', 'cancelled', 'refused')


def handler(identity):
    class LaunchProvider(identity.Identity):
        links = {}
        creates = 0
        reads = 0

        def admin(self):
            return hmac.compare_digest(self.headers.get('Authorization', ''),
                                       'Bearer ' + self.config['admin_bearer'])

        def state(self):
            return {'creates': type(self).creates, 'reads': type(self).reads,
                    'links': {key: row['status'] for key, row in self.links.items()}}

        def do_POST(self):
            if self.path.startswith('/fixture/launch/'):
                if not self.admin():
                    return self.reply(403, {'error': 'control_denied'})
                match = re.fullmatch(r'/fixture/launch/(link_[a-f0-9]{32})/([a-z]+)', self.path)
                if (not match or match[2] not in STATES
                        or self.headers.get('Content-Length', '0') != '0'):
                    return self.reply(400, {'error': 'control_invalid'})
                with self.lock:
                    if match[1] not in self.links:
                        return self.reply(404, {'error': 'link_missing'})
                    self.links[match[1]]['status'] = match[2]
                    state = self.state()
                return self.reply(200, state)
            if self.path != '/api/v1/connect-links':
                return super().do_POST()
            if not self.bearer():
                return self.reply(401, {'error': 'unauthorized'})
            try:
                body = json.loads(self.body())
                if body != {'service_slug': SERVICE, 'expires_in': 900,
                            'callback_url': 'https://api.localhost.direct/connection-return'}:
                    raise ValueError('connect_request')
            except (ValueError, UnicodeDecodeError):
                return self.reply(400, {'error': 'connect_request'})
            with self.lock:
                if len(self.links) >= 10:
                    return self.reply(429, {'error': 'finite_link_limit'})
                key = 'link_' + secrets.token_hex(16)
                expiry = (datetime.now(timezone.utc) + timedelta(seconds=900)).isoformat()
                self.links[key] = {'id': key, 'service_slug': SERVICE, 'status': 'pending',
                                   'expires_at': expiry}
                type(self).creates += 1
            return self.reply(201, {'id': key, 'expires_at': expiry,
                'connect_url': 'https://identity.local.test/connect/nyx_clk_' + secrets.token_hex(32)})

        def do_GET(self):
            if self.path == '/fixture/launch/state':
                if not self.admin():
                    return self.reply(403, {'error': 'control_denied'})
                with self.lock:
                    state = self.state()
                return self.reply(200, state)
            special = (self.path.startswith('/api/v1/connect-links/') or self.path in (
                '/api/v1/user-services', '/api/v1/services', '/api/v1/keys/fixture-matrix-connect'))
            if not special:
                return super().do_GET()
            if not self.bearer():
                return self.reply(401, {'error': 'unauthorized'})
            service = {'id': 'fixture-matrix-connect', 'slug': SERVICE, 'is_active': True}
            if self.path == '/api/v1/user-services':
                with self.lock:
                    connected = any(row['status'] == 'completed' for row in self.links.values())
                return self.reply(200, {'services': identity.SERVICES + ([service] if connected else [])})
            if self.path == '/api/v1/services':
                return self.reply(200, {'services': [{'slug': SERVICE, 'name': 'Matrix connection',
                                                    'service_type': 'http', 'is_active': True}]})
            if self.path == '/api/v1/keys/fixture-matrix-connect':
                return self.reply(200, {**service, 'status': 'active', 'connected': True,
                    'credential_missing': False, 'credential_source': {'type': 'personal'},
                    'credential_type': 'node_managed', 'node_id': 'fixture-node', 'node_status': 'online'})
            key = self.path.removeprefix('/api/v1/connect-links/')
            with self.lock:
                type(self).reads += 1
                row = deepcopy(self.links.get(key))
            if row is None or row['status'] == 'refused':
                return self.reply(404, {'error': 'link_missing'})
            if row['status'] == 'completed':
                row['connected_service'] = {'id': service['id'], 'slug': SERVICE}
            return self.reply(200, row)
    return LaunchProvider


def manifests(objects):
    from migration_sandbox_fixture import manifests as without_runtime_mutations
    objects = without_runtime_mutations(objects)
    selected = {(obj['kind'], obj['metadata']['name']): obj for obj in objects}
    container = selected[('Deployment', 'identity')]['spec']['template']['spec']['containers'][0]
    if container['command'] != ['/usr/local/bin/python3', '-B', '/code/identity.py']:
        raise ValueError('launch identity command already customized')
    selected[('ConfigMap', 'identity-code')]['data']['migration_launch_provider.py'] = Path(__file__).read_text()
    container['command'][-1] = '/code/migration_launch_provider.py'
    return objects


if __name__ == '__main__':
    import identity
    provider = handler(identity)
    provider.config = json.loads(Path('/private/identity.json').read_text())
    server = ThreadingHTTPServer(('0.0.0.0', 8080), provider)
    server.daemon_threads = True
    server.serve_forever(poll_interval=0.2)
