#!/usr/bin/env python3
"""Finite synthetic access credentials; exact issued bytes authorize each call.

The JWT-shaped payload only passes the application's negative shape filter.
The provider never authenticates by decoding it or accepting an arbitrary JWT.
"""
import base64
from copy import deepcopy
import hashlib
import hmac
from http.server import ThreadingHTTPServer
import json
from pathlib import Path
import secrets
import time

ISSUER = 'http://identity:8080'
SLOTS = tuple(f'{writer}/{index}' for writer in ('old-write', 'new-write') for index in range(2))


def metadata(token, subject, expiry):
    credential = hashlib.sha256(b'organization-caller\0' + token.encode()).hexdigest()
    identity = json.dumps(['cma-api-bearer-custody-v1', ISSUER, subject, credential],
                          separators=(',', ':')).encode()
    return {'issuer': ISSUER, 'subject': subject, 'credential_id': credential,
            'custody_id': hashlib.sha256(identity).hexdigest(),
            'expires_at': expiry, 'token_bytes': len(token.encode())}


def issue(subject, expiry):
    def encoded(value):
        return base64.urlsafe_b64encode(json.dumps(value, separators=(',', ':')).encode()).decode().rstrip('=')
    return '.'.join([encoded({'typ': 'JWT', 'alg': 'HS256'}),
                     encoded({'sub': subject, 'token_type': 'access', 'exp': expiry,
                              'jti': secrets.token_hex(16)}), secrets.token_urlsafe(32)])


def handler(identity):
    class CustodyProvider(identity.Identity):
        slots = {}

        def actor(self, token):
            with self.lock:
                if type(self.config.get('expires_at')) is int and self.config['expires_at'] > time.time():
                    for issued in self.slots.values():
                        if (issued['expires_at'] == self.config['expires_at']
                                and hmac.compare_digest(token, issued['token'])):
                            return identity.OWNER, identity.PERMISSIONS
            return super().actor(token)

        def do_POST(self):
            if not self.path.startswith('/fixture/api-custody/'):
                return super().do_POST()
            if not hmac.compare_digest(self.headers.get('Authorization', ''),
                                       'Bearer ' + self.config['admin_bearer']):
                return self.reply(403, {'error': 'custody_control_denied'})
            slot = self.path.removeprefix('/fixture/api-custody/')
            if slot not in SLOTS or self.headers.get('Content-Length', '0') != '0':
                return self.reply(400, {'error': 'custody_control_invalid'})
            expiry = self.config.get('expires_at')
            if type(expiry) is not int or not time.time() < expiry <= time.time() + 7200:
                return self.reply(409, {'error': 'custody_expiry_invalid'})
            with self.lock:
                if slot not in self.slots:
                    self.slots[slot] = {'token': issue(identity.OWNER, expiry), 'expires_at': expiry}
                issued = self.slots[slot]
                if issued['expires_at'] != expiry:
                    return self.reply(409, {'error': 'custody_expiry_changed'})
                token = issued['token']
            return self.reply(200, {'slot': slot, 'access_token': token,
                                   'metadata': metadata(token, identity.OWNER, expiry)})
    return CustodyProvider


def manifests(objects):
    objects = deepcopy(objects)
    selected = {(o['kind'], o['metadata']['name']): o for o in objects}
    if selected[('ConfigMap', 'api-env')]['data'].get('CHRONO_AUTH_MODE') != 'provider':
        raise ValueError('API custody fixture requires provider authentication')
    container = selected[('Deployment', 'identity')]['spec']['template']['spec']['containers'][0]
    if container['command'] != ['/usr/local/bin/python3', '-B', '/code/identity.py']:
        raise ValueError('API custody provider already customized')
    selected[('ConfigMap', 'identity-code')]['data']['migration_api_custody_provider.py'] = Path(__file__).read_text()
    container['command'][-1] = '/code/migration_api_custody_provider.py'
    return objects


if __name__ == '__main__':
    import identity
    provider = handler(identity)
    provider.config = json.loads(Path('/private/identity.json').read_text())
    server = ThreadingHTTPServer(('0.0.0.0', 8080), provider)
    server.daemon_threads = True
    server.serve_forever(poll_interval=0.2)
