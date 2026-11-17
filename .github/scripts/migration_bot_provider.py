#!/usr/bin/env python3
"""Opt-in Bot endpoints layered over the existing synthetic identity fixture."""
from datetime import datetime
import hmac
from http.server import ThreadingHTTPServer
import json
from pathlib import Path
import re
import secrets
import time
from urllib.parse import urlsplit

from migration_bot_gateway import ContractError, Gateway, require


def handler(identity_module):
    class BotProvider(identity_module.Identity):
        gateway = Gateway(identity_module.OWNER)
        keys = {}
        key_effects = {'mint': 0, 'revoke': 0}

        def reply(self, status, body):
            path = urlsplit(self.path).path
            if not path.startswith('/v1/'):
                return super().reply(status, body)
            raw = json.dumps(body, separators=(',', ':')).encode()
            self.send_response(status)
            for name, value in [('Content-Type', 'application/json'), ('Cache-Control', 'no-store'),
                                ('Content-Length', str(len(raw)))]:
                self.send_header(name, value)
            self.end_headers()
            self.wfile.write(raw)
            # The base fixture only has key DELETEs. Gateway operations need
            # their own route labels; neither raw IDs nor credentials are logged.
            parts = path.strip('/').split('/')
            route = '/' + '/'.join(parts[:2] + ([':id'] if len(parts) > 2 else []) + parts[3:])
            print(json.dumps({'at': time.time(), 'method': self.command, 'route': route,
                              'status': status}), flush=True)

        def owner(self):
            return hmac.compare_digest(self.headers.get('Authorization', ''),
                                       'Bearer ' + self.config['bearer'])

        def end_headers(self):
            self.send_header('cmaeg-protocol-version', '1')
            super().end_headers()

        def do_GET(self):
            if self.path.startswith('/v1/'):
                return self.gateway_request()
            if self.path == '/fixture/bot-state':
                if not hmac.compare_digest(self.headers.get('Authorization', ''),
                                           'Bearer ' + self.config['admin_bearer']):
                    return self.reply(401, {'error': 'unauthorized'})
                with self.lock:
                    count = len(self.keys)
                    effects = dict(self.key_effects)
                return self.reply(200, {**self.gateway.summary(), 'keys': count, 'key_effects': effects})
            if self.path == '/api/v1/user-services':
                if not self.owner():
                    return self.reply(401, {'error': 'unauthorized'})
                return self.reply(200, [{**row, 'credential_source': {'type': 'personal'}}
                                        for row in identity_module.SERVICES])
            if (self.path == '/api/v1/channel-conversations'
                    or self.path.startswith('/api/v1/api-keys')):
                if not self.owner():
                    return self.reply(401, {'error': 'unauthorized'})
                with self.lock:
                    if self.path == '/api/v1/channel-conversations':
                        return self.reply(200, {'conversations': [], 'total': 0})
                    if self.path == '/api/v1/api-keys':
                        return self.reply(200, {'keys': list(self.keys.values())})
                    row = self.keys.get(self.path.removeprefix('/api/v1/api-keys/'))
                    return self.reply(200 if row else 404, row or {})
            return super().do_GET()

        def do_POST(self):
            if self.path.startswith('/v1/'):
                return self.gateway_request()
            if self.path != '/api/v1/api-keys':
                return super().do_POST()
            if not self.owner():
                return self.reply(401, {'error': 'unauthorized'})
            try:
                body = json.loads(self.body())
                plan = identity_module.scope_plan(body['allowed_service_ids'])
                fields = {'name', 'scopes', 'expires_at', 'allowed_service_ids', 'allowed_node_ids',
                          'allow_all_services', 'allow_all_nodes', 'allow_auto_connected_services',
                          'scope_plan_digest'}
                require(set(body) == fields and body['scopes'] == 'proxy read', 'key_fields')
                require(re.fullmatch(r'cma:bot:bot_[a-f0-9]{32}:agent:[1-9][0-9]*', body['name']),
                        'key_namespace')
                require(body['allowed_node_ids'] == ['fixture-node']
                        and all(body[k] is False for k in ['allow_all_services', 'allow_all_nodes',
                                                          'allow_auto_connected_services'])
                        and body['scope_plan_digest'] == plan['normalized_grant_digest'], 'key_grant')
                require(datetime.fromisoformat(body['expires_at'].replace('Z', '+00:00')).timestamp()
                        > time.time(), 'key_expiry')
                identifier = 'fixture-key-' + secrets.token_hex(16)
                row = {**body, 'id': identifier, 'is_active': True, 'state_version': 1,
                       'credential_source': {'type': 'personal'}, 'purpose': 'general',
                       'scheduled_write_enabled': False}
                with self.lock:
                    require(len(self.keys) < 16, 'key_bound')
                    self.keys[identifier] = row
                    self.key_effects['mint'] += 1
                # The full synthetic credential is returned once, never saved in
                # metadata or logs. The gateway retains only its SHA-256 hash.
                return self.reply(200, {**row, 'full_key': 'nyx_' + secrets.token_hex(32)})
            except (ValueError, KeyError, TypeError, OverflowError):
                return self.reply(400, {'error': 'bot_key_contract_refused'})

        def do_PUT(self):
            return self.gateway_request()

        def do_DELETE(self):
            if self.path.startswith('/v1/'):
                return self.gateway_request()
            if not self.owner():
                return self.reply(401, {'error': 'unauthorized'})
            if not re.fullmatch(r'/api/v1/api-keys/fixture-key-[a-f0-9]{32}', self.path):
                return self.reply(404, {'error': 'unsupported_route'})
            with self.lock:
                removed = self.keys.pop(self.path.rsplit('/', 1)[1], None)
                if removed:
                    self.key_effects['revoke'] += 1
            return self.reply(200 if removed else 404, {'removed': removed is not None})

        def gateway_request(self):
            if not self.owner() or self.headers.get('X-CMAEG-Owner'):
                return self.reply(403, {'error': 'owner_scope'})
            try:
                raw = self.body() if self.command in ('POST', 'PUT') else b''
                body = json.loads(raw) if raw else None
                code, response = self.gateway.request(self.command, self.path, body)
                return self.reply(code, response)
            except (ContractError, ValueError, KeyError, TypeError):
                return self.reply(400, {'error': 'gateway_contract_refused'})

    return BotProvider


def manifests(objects):
    """Add only scoped local dependency endpoints to an existing provider manifest."""
    selected = {(obj['kind'], obj['metadata']['name']): obj for obj in objects}
    config = selected[('ConfigMap', 'api-env')]['data']
    require(config.get('CHRONO_AUTH_MODE') == 'provider', 'provider_auth_required')
    config.update(CHRONO_CMAEG_BASE_URL='http://identity:8080',
                  CHRONO_CMAEG_PUBLIC_URL='https://gateway.matrix.invalid',
                  CHRONO_CMAEG_BINDING_PREPARATION_ENABLED='false')
    code = selected[('ConfigMap', 'identity-code')]['data']
    for name in ['migration_bot_gateway.py', 'migration_bot_provider.py']:
        code[name] = Path(__file__).with_name(name).read_text()
    container = selected[('Deployment', 'identity')]['spec']['template']['spec']['containers'][0]
    require(container['command'] == ['/usr/local/bin/python3', '-B', '/code/identity.py'], 'identity_command')
    container['command'][-1] = '/code/migration_bot_provider.py'
    return objects


if __name__ == '__main__':
    import identity
    provider = handler(identity)
    provider.config = json.loads(Path('/private/identity.json').read_text())
    server = ThreadingHTTPServer(('0.0.0.0', 8080), provider)
    server.daemon_threads = True
    server.serve_forever(poll_interval=0.2)
