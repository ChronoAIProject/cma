#!/usr/bin/env python3
"""Local NyxID dependency only. Never implements Kubernetes or assignment."""
import base64
from datetime import datetime, timezone
import hashlib
import hmac
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import re
import secrets
import ssl
import threading
import time
from urllib.parse import parse_qs, urlencode, urlsplit

OWNER = 'native-am-fixture-owner'
ORGANIZATION = {'id': 'fixture-organization', 'your_role': 'member', 'display_name': 'Synthetic organization'}
SERVICES = [{'id': 'fixture-llm', 'slug': 'chrono-llm-public', 'is_active': True},
            {'id': 'fixture-ornn', 'slug': 'ornn-api', 'is_active': True},
            {'id': 'fixture-access', 'slug': 'fixture-access-service', 'is_active': True}]
PERMISSIONS = ['cma:sandbox:create', 'cma:sandbox:use',
               'cma:sandbox:delete', 'cma:profile:manage', 'cma:bot:manage', 'cma:trigger:publish', 'cma:trigger:launch']
ADMIN_PERMISSIONS = PERMISSIONS + ['cma:admin:sandboxes', 'cma:admin:events', 'cma:admin:activity', 'cma:admin:users', 'cma:admin:control', 'cma:admin:settings']


def scope_plan(selected):
    if (not isinstance(selected, list) or not selected or len(set(selected)) != len(selected)
            or not set(selected) <= {x['id'] for x in SERVICES}):
        raise ValueError('unsupported_scope')
    selected = sorted(selected)
    owner = {'id': OWNER, 'type': 'personal'}
    plan = dict(authority='nyxid', contract_version='1', policy_version='api-key-scope-v1',
                authenticated_actor=owner, intended_key_owner=owner,
                services=[{'user_service_id': x, 'resource_owner': owner,
                           'node_grant': {'type': 'required', 'node_ids': ['fixture-node']}}
                          for x in selected],
                allowed_service_ids=selected, allowed_node_ids=['fixture-node'],
                evaluated_at=datetime.now(timezone.utc).isoformat(), key_purpose='general',
                freshness={'mode': 'mutation_revalidated_snapshot',
                           'precondition_field': 'scope_plan_digest', 'post_creation_drift': 'fail_closed'},
                completeness={'list_complete': True, 'no_duplicates': True,
                              'route_candidate_basis': 'active_configured_routes',
                              'transient_node_state_excluded': True})
    # Exact length-prefixed v1 digest in dfc2 auth/keys/scope_plan.rs.
    digest = hashlib.sha256()
    def count(n):
        digest.update(n.to_bytes(8, 'big'))
    def field(value):
        raw = value.encode()
        count(len(raw))
        digest.update(raw)
    for value in ['nyxid-api-key-scope-plan-digest', 'nyxid', '1', 'api-key-scope-v1',
                  OWNER, 'personal', OWNER, 'personal', 'services']:
        field(value)
    count(len(selected))
    for service in selected:
        for value in ['service', service, OWNER, 'required']:
            field(value)
        count(1)
        field('fixture-node')
    field('allowed_service_ids')
    count(len(selected))
    for service in selected:
        field(service)
    field('allowed_node_ids')
    count(1)
    field('fixture-node')
    plan['normalized_grant_digest'] = 'sha256:' + digest.hexdigest()
    return plan


class Identity(BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.0'
    config = None
    keys = {}
    codes = {}
    outsider_is_member = False
    lock = threading.Lock()

    def log_message(self, *_):
        pass

    def reply(self, status, body):
        raw = json.dumps(body, separators=(',', ':')).encode()
        self.send_response(status)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Cache-Control', 'no-store')
        self.send_header('Content-Length', str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)
        # Route and status only, never headers/bodies/credential or key IDs.
        route = '/api/v1/api-keys/:id' if self.command == 'DELETE' else urlsplit(self.path).path
        print(json.dumps({'at': time.time(), 'method': self.command, 'route': route,
                          'status': status}), flush=True)

    def body(self):
        self.connection.settimeout(5)
        size = int(self.headers.get('Content-Length', '0'))
        if not 0 <= size <= 16384:
            raise ValueError('body_bound')
        return self.rfile.read(size)

    def bearer(self):
        return any(hmac.compare_digest(self.headers.get('Authorization', ''), 'Bearer ' + self.config[k]) for k in ['bearer', 'admin_bearer'])

    def actor(self, token):
        for key, subject, permissions in [
            ('bearer', OWNER, PERMISSIONS),
            ('admin_bearer', 'native-am-fixture-admin', ADMIN_PERMISSIONS),
            ('outsider_bearer', 'native-am-fixture-outsider', PERMISSIONS),
        ]:
            expected = self.config.get(key)
            if expected and hmac.compare_digest(token, expected):
                return subject, permissions
        return None

    def do_GET(self):
        if self.path in ('/fixture/ornn-grant', '/fixture/llm-grant'):
            service = 'fixture-ornn' if self.path == '/fixture/ornn-grant' else 'fixture-llm'
            auth = self.headers.get('Authorization', '')
            with self.lock:
                granted = any(service in value.get('service_ids', ())
                              and hmac.compare_digest(auth, 'Bearer ' + value['credential'])
                              for value in self.keys.values())
            return self.reply(200 if granted else 401, {'granted': granted})
        if urlsplit(self.path).path == '/authorize':
            params = parse_qs(urlsplit(self.path).query)
            try:
                if (params['client_id'] != [self.config['client_id']]
                    or params['redirect_uri'] != ['https://api.localhost.direct/auth/callback']
                    or params['response_type'] != ['code'] or params['code_challenge_method'] != ['S256']
                    or len(params['state'][0]) > 256 or len(params['code_challenge'][0]) != 43):
                    raise ValueError('authorization_contract')
                code = secrets.token_hex(32)
                with self.lock:
                    if self.codes:
                        raise ValueError('single_authorization_bound')
                    self.codes[code] = params['code_challenge'][0]
                location = 'https://api.localhost.direct/auth/callback?' + urlencode({'state': params['state'][0], 'code': code})
                self.send_response(302)
                self.send_header('Location', location)
                self.send_header('Content-Length', '0')
                self.end_headers()
                print(json.dumps({'at': time.time(), 'method': 'GET', 'route': '/authorize', 'status': 302}), flush=True)
                return
            except (KeyError, ValueError, IndexError):
                return self.reply(400, {'error': 'authorization_contract'})
        if self.path == '/fixture/health':
            return self.reply(200, {'ok': True, 'native_provider': False})
        if self.path == '/.well-known/openid-configuration':
            base = 'http://identity:8080'
            return self.reply(200, {x + '_endpoint': base + '/' + y for x, y in
                                   [('authorization', 'authorize'), ('token', 'token'),
                                    ('userinfo', 'userinfo'), ('introspection', 'introspect')]})
        if self.path == '/userinfo':
            header = self.headers.get('Authorization', '')
            actor = self.actor(header[7:]) if header.startswith('Bearer ') else None
            return self.reply(200, {'sub': actor[0], 'name': 'Local native qualification',
                                    'permissions': actor[1]}) if actor else self.reply(401, {'error': 'unauthorized'})
        org_authorization = '/api/v1/orgs/' + ORGANIZATION['id'] + '/authorization'
        if self.path in ('/api/v1/orgs', org_authorization):
            authorization = self.headers.get('Authorization', '')
            actor = self.actor(authorization[7:]) if authorization.startswith('Bearer ') else None
            if actor is None:
                return self.reply(401, {'error': 'unauthorized'})
            with self.lock:
                member = actor[0] == 'native-am-fixture-outsider' and self.outsider_is_member
            if self.path == '/api/v1/orgs':
                return self.reply(200, {'orgs': [ORGANIZATION] if member else []})
            return self.reply(200, ORGANIZATION) if member else self.reply(404, {'error': 'not_member'})
        if not self.bearer():
            return self.reply(401, {'error': 'unauthorized'})
        if self.path in ['/api/v1/services', '/api/v1/user-services']:
            return self.reply(200, SERVICES)
        if self.path.startswith('/api/v1/keys/'):
            row = next((x for x in SERVICES if self.path == '/api/v1/keys/' + x['id']), None)
            if row:
                return self.reply(200, {**row, 'status': 'active', 'connected': True,
                    'credential_missing': False, 'credential_source': {'type': 'personal'},
                    'credential_type': 'node_managed', 'node_id': 'fixture-node', 'node_status': 'online'})
        if self.path == '/api/v1/api-keys':
            with self.lock:
                rows = [{'id': k, 'name': v['name']} for k, v in self.keys.items()]
            return self.reply(200, {'keys': rows})
        return self.reply(404, {'error': 'unsupported_route'})

    def do_POST(self):
        try:
            raw = self.body()
            if self.path == '/fixture/organization-membership':
                admin = 'Bearer ' + self.config['admin_bearer']
                if not hmac.compare_digest(self.headers.get('Authorization', ''), admin):
                    return self.reply(403, {'error': 'fixture_admin_required'})
                body = json.loads(raw)
                if not isinstance(body, dict) or set(body) != {'member'} or type(body['member']) is not bool:
                    raise ValueError('organization_membership_contract')
                with self.lock:
                    type(self).outsider_is_member = body['member']
                return self.reply(200, {'member': body['member']})
            if self.path == '/token':
                form = parse_qs(raw.decode(), strict_parsing=True)
                if (form.get('client_id') != [self.config['client_id']]
                    or form.get('client_secret') != [self.config['client_secret']]):
                    return self.reply(401, {'error': 'invalid_client'})
                grant = form.get('grant_type')
                if grant == ['authorization_code']:
                    with self.lock:
                        expected = self.codes.pop(form.get('code', [''])[0], None)
                    actual = base64.urlsafe_b64encode(hashlib.sha256(form.get('code_verifier', [''])[0].encode()).digest()).decode().rstrip('=')
                    if (expected is None or not hmac.compare_digest(expected, actual)
                        or form.get('redirect_uri') != ['https://api.localhost.direct/auth/callback']):
                        return self.reply(400, {'error': 'invalid_grant'})
                elif grant == ['refresh_token']:
                    if form.get('refresh_token') != [self.config['refresh_token']]:
                        return self.reply(400, {'error': 'invalid_grant'})
                else:
                    return self.reply(400, {'error': 'unsupported_grant'})
                return self.reply(200, {'access_token': self.config['bearer'], 'token_type': 'Bearer',
                    'refresh_token': self.config['refresh_token'], 'expires_in': 3600,
                    'scope': 'openid profile email roles proxy offline_access'})
            if self.path == '/introspect':
                form = parse_qs(raw.decode(), strict_parsing=True)
                actor = self.actor(form.get('token', [''])[0])
                expected = {'client_id': self.config['client_id'],
                            'client_secret': self.config['client_secret'], 'token_type_hint': 'access_token'}
                if actor is None or any(not hmac.compare_digest(form.get(k, [''])[0], v) for k, v in expected.items()):
                    return self.reply(401, {'active': False})
                return self.reply(200, {'active': True, 'sub': actor[0], 'token_type': 'access',
                                       'exp': self.config['expires_at'], 'permissions': actor[1]})
            if not self.bearer():
                return self.reply(401, {'error': 'unauthorized'})
            body = json.loads(raw)
            if self.path == '/api/v1/api-keys/scope-plan':
                if set(body) != {'selected_service_ids'}:
                    raise ValueError('scope_fields')
                return self.reply(200, scope_plan(body['selected_service_ids']))
            if self.path == '/api/v1/api-keys':
                fields = {'name', 'scopes', 'expires_at', 'allowed_service_ids', 'allowed_node_ids',
                          'allow_all_services', 'allow_all_nodes', 'allow_auto_connected_services', 'scope_plan_digest'}
                plan = scope_plan(body['allowed_service_ids'])
                if (set(body) != fields or body['scopes'] != 'proxy read'
                    or not re.fullmatch(r'cma:' + re.escape(self.config.get('deployment_id', 'native-am-1115'))
                                        + r':[A-Za-z0-9_.-]{1,128}', body['name'])
                    or body['allowed_node_ids'] != ['fixture-node']
                    or any(body[x] is not False for x in ['allow_all_services', 'allow_all_nodes', 'allow_auto_connected_services'])
                    or body['scope_plan_digest'] != plan['normalized_grant_digest']
                    or datetime.fromisoformat(body['expires_at'].replace('Z', '+00:00')).timestamp() <= time.time()):
                    raise ValueError('mint_contract')
                identifier = 'fixture-key-' + secrets.token_hex(16)
                credential = 'local-' + secrets.token_hex(32)
                with self.lock:
                    if len(self.keys) >= 16:
                        raise ValueError('mint_bound')
                    self.keys[identifier] = {'name': body['name'], 'credential': credential,
                                             'service_ids': body['allowed_service_ids']}
                return self.reply(200, dict(id=identifier, full_key=credential,
                    scopes=body['scopes'], state_version=1, allowed_service_ids=body['allowed_service_ids'],
                    allowed_node_ids=body['allowed_node_ids'], allow_all_services=False,
                    allow_all_nodes=False, allow_auto_connected_services=False))
            return self.reply(404, {'error': 'unsupported_route'})
        except (ValueError, KeyError, TypeError, OverflowError):
            return self.reply(400, {'error': 'fixture_contract_refused'})

    def do_DELETE(self):
        if not self.bearer():
            return self.reply(401, {'error': 'unauthorized'})
        if not re.fullmatch(r'/api/v1/api-keys/fixture-key-[a-f0-9]{32}', self.path):
            return self.reply(404, {'error': 'unsupported_route'})
        with self.lock:
            removed = self.keys.pop(self.path.rsplit('/', 1)[1], None)
        return self.reply(200 if removed else 404, {'removed': removed is not None})


if __name__ == '__main__':
    Identity.config = json.loads(Path('/private/identity.json').read_text())
    certificate = Path('/private/provider.crt')
    if certificate.is_file():
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.minimum_version = ssl.TLSVersion.TLSv1_2
        context.load_cert_chain(certificate, '/private/provider.key')
        secure = ThreadingHTTPServer(('0.0.0.0', 8443), Identity)
        secure.socket = context.wrap_socket(secure.socket, server_side=True)
        threading.Thread(target=secure.serve_forever, kwargs={'poll_interval': 0.2}, daemon=True).start()
    server = ThreadingHTTPServer(('0.0.0.0', 8080), Identity)
    server.daemon_threads = True
    server.serve_forever(poll_interval=0.2)
