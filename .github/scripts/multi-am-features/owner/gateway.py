"""Start the tested Event Gateway with its own authenticated fixture database."""
import base64
import hashlib
import json
import re
import secrets

from common import require_feature_fixture
from legacy_manifests import metadata, pod, resource

DATABASE = 'cma_eg_fixture'
PORT = 8090


def configured_role(role, name):
    if (not isinstance(role, dict) or role.get('createRole') != name or role.get('roles') != []
            or not role.get('privileges') or any(
                set(grant['resource']) != {'db', 'collection'} or grant['resource']['db'] != DATABASE
                for grant in role['privileges'])):
        raise ValueError('gateway_fixture_role_unbound')
    return role


def initialize_gateway(fixture):
    require_feature_fixture(fixture)
    image = fixture.ready.get('expected_eg_image', '')
    if not re.fullmatch(r'localhost/multi-am-cmaeg@sha256:[a-f0-9]{64}', image):
        raise ValueError('gateway_fixture_image_unbound')
    client = fixture.client
    # A one-shot process from the same image emits the exact role or applies
    # its schema. None of these steps may fall back to another image.
    def completed(name, arguments, env):
        client.apply([pod(name, image, resource('500m', '256Mi', '50m', '32Mi'),
                          args=arguments, env=env)])

        def succeeded():
            phase = client.kjson(['get', 'pod', name, '-o', 'json'])['status']['phase']
            if phase == 'Failed':
                raise RuntimeError(name.replace('-', '_') + '_failed')
            return phase == 'Succeeded'

        client.wait(succeeded, 90, name.replace('-', '_') + '_timeout')

    database_env = [{'name': 'CMAEG_MONGODB_DATABASE', 'value': DATABASE}]
    fixture.mark_initialization_step('gateway_fixture_roles')
    accounts = []
    for kind, role_name in [('runtime', 'cma_eg_runtime'), ('migration', 'cma_eg_migrator')]:
        name = f'gateway-{kind}-role'
        completed(name, [f'--print-{kind}-role'], database_env)
        role = configured_role(json.loads(client.k(['logs', name, '--limit-bytes=32768'])), role_name)
        accounts.append({'role': role, 'user': f'gateway-{kind}', 'password': secrets.token_urlsafe(32)})

    script = (f'const gateway = db.getSiblingDB({json.dumps(DATABASE)});'
              'if (gateway.getCollectionNames().length !== 0) throw Error("gateway_database_not_empty");'
              f'for (const account of {json.dumps(accounts)}) {{'
              'if (gateway.runCommand(account.role).ok !== 1) throw Error("gateway_role_create_failed");'
              'gateway.createUser({user: account.user, pwd: account.password, roles: [account.role.createRole]});'
              '}; print(JSON.stringify(true));')
    if fixture.mongo(script) is not True:
        raise RuntimeError('gateway_fixture_database_unbound')

    def uri(account):
        return f"mongodb://{account['user']}:{account['password']}@mongo:27017/{DATABASE}?directConnection=true"

    setup = {'apiVersion': 'v1', 'kind': 'ConfigMap', 'metadata': metadata('gateway-migration'),
             'data': {'CMAEG_MONGODB_URI': uri(accounts[1])}}
    client.apply([setup])
    fixture.mark_initialization_step('gateway_fixture_schema')
    completed('gateway-schema', ['--migrate'], [*database_env, {'name': 'CMAEG_MONGODB_URI', 'valueFrom': {
        'configMapKeyRef': {'name': 'gateway-migration', 'key': 'CMAEG_MONGODB_URI'}}}])

    namespace = fixture.ready['namespace']
    public_url = 'https://gateway.example.test'
    identity = fixture.secret['gateway_identity']
    env = {
        'CMAEG_LISTEN': f'0.0.0.0:{PORT}', 'CMAEG_MONGODB_DATABASE': DATABASE,
        'CMAEG_PUBLIC_URL': public_url, 'CMAEG_MONGODB_URI': uri(accounts[0]),
        'CMAEG_CMA_BASE_URL': f'http://am-public.{namespace}.svc:8080',
        'CMAEG_NYXID_BASE_URL': 'https://identity.localhost.direct',
        'CMAEG_CMA_SERVICE_TOKEN': secrets.token_urlsafe(32),
        'CMAEG_NYXID_CLIENT_ID': identity['client_id'],
        'CMAEG_NYXID_CLIENT_SECRET': identity['client_secret'],
        'CMAEG_MASTER_KEYS': '1:' + base64.b64encode(secrets.token_bytes(32)).decode().rstrip('='),
    }
    config = {'apiVersion': 'v1', 'kind': 'ConfigMap', 'metadata': metadata('gateway-environment'), 'data': env}
    gateway = pod('gateway', image, resource('1', '384Mi', '50m', '64Mi'),
                  envFrom=[{'configMapRef': {'name': 'gateway-environment'}}],
                  readinessProbe={'httpGet': {'path': '/readyz', 'port': PORT}, 'periodSeconds': 2})
    fixture.mark_initialization_step('gateway_fixture_ready')
    client.apply([config, gateway])

    def ready():
        observed = client.kjson(['get', 'pod', 'gateway', '-o', 'json'])
        status = observed['status']
        if status.get('phase') == 'Failed':
            raise RuntimeError('gateway_fixture_process_exited')
        if observed['spec']['containers'][0]['image'] != image:
            raise RuntimeError('gateway_fixture_process_image_changed')
        return any(row.get('name') == 'gateway' and row.get('ready') is True
                   for row in status.get('containerStatuses', []))

    client.wait(ready, 90, 'gateway_fixture_process_not_ready')
    origin = client.forward('gateway', PORT)
    status, response = client.http(origin, 'GET', '/readyz')
    if status != 200 or response.get('ready') is not True:
        raise RuntimeError('gateway_fixture_ready_readback')
    document_status, headers, body = client.http_raw(origin, 'GET', '/docs')
    if (document_status != 200 or not headers.get('Content-Type', '').startswith('text/html')
            or any(f'id="{section}"'.encode() not in body for section in ('integration', 'architecture', 'protocol'))):
        raise RuntimeError('gateway_fixture_docs_readback')
    verify_gateway_auth(fixture, origin)
    fixture.record('event_gateway_ready', manifest=image.split('@', 1)[1], docs_sha256=hashlib.sha256(body).hexdigest())
    return origin


def verify_gateway_auth(fixture, origin):
    path = '/v1/channel-capabilities'
    status, value = fixture.client.http(origin, 'GET', path,
        headers={'Authorization': 'Bearer ' + fixture.secret['bearer']})
    if (status != 200 or not isinstance(value, dict) or not isinstance(value.get('platforms'), list)
            or not isinstance(value.get('relay_authoring'), dict)):
        raise RuntimeError('gateway_fixture_actor_readback')
    outsider, _ = fixture.client.http(origin, 'GET', path,
        headers={'Authorization': 'Bearer ' + fixture.secret['outsider_bearer']})
    anonymous, _ = fixture.client.http(origin, 'GET', path)
    if outsider != 403 or anonymous != 401:
        raise RuntimeError('gateway_fixture_authority_refusal')
