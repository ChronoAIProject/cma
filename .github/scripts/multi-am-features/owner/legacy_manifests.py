"""Named local dependencies; native runtime objects are generated solely by AM."""
import base64
import json
from pathlib import Path
import secrets
import time

NS = 'cma-native-am'
DEPLOYMENT = None
AM = None
BASELINE = None
MONGO = 'docker.io/library/mongo@sha256:42545cf0629db918d1a24a2df8bbca6926ed87bfabf59afc85008dd948448cc7'


def metadata(name):
    return {'name': name, 'namespace': NS, 'labels': {'qualification': DEPLOYMENT, 'app': name}}


def resource(cpu, memory, request_cpu=None, request_memory=None):
    return {'limits': {'cpu': cpu, 'memory': memory},
            'requests': {'cpu': request_cpu or cpu, 'memory': request_memory or memory}}


def pod(name, image, resources, **container):
    return {'apiVersion': 'v1', 'kind': 'Pod', 'metadata': metadata(name), 'spec': {
        'restartPolicy': 'Never', 'automountServiceAccountToken': False,
        'terminationGracePeriodSeconds': 15,
        'containers': [{'name': name, 'image': image, 'imagePullPolicy': 'Never',
                        'resources': resources, **container}]}}


def service(name, port):
    return {'apiVersion': 'v1', 'kind': 'Service', 'metadata': metadata(name),
            'spec': {'selector': {'app': name}, 'ports': [{'port': port, 'targetPort': port}]}}


def credentials():
    claims = json.dumps({'sub': 'native-am-fixture-owner', 'token_type': 'access',
                         'exp': int(time.time()) + 3600}, separators=(',', ':')).encode()
    bearer = 'e30.' + base64.urlsafe_b64encode(claims).decode().rstrip('=') + '.' + secrets.token_hex(32)
    claims = json.dumps({'sub': 'native-am-fixture-admin', 'token_type': 'access',
                         'exp': int(time.time()) + 3600}, separators=(',', ':')).encode()
    admin_bearer = 'e30.' + base64.urlsafe_b64encode(claims).decode().rstrip('=') + '.' + secrets.token_hex(32)
    claims = json.dumps({'sub': 'native-am-fixture-outsider', 'token_type': 'access',
                         'exp': int(time.time()) + 3600}, separators=(',', ':')).encode()
    outsider_bearer = 'e30.' + base64.urlsafe_b64encode(claims).decode().rstrip('=') + '.' + secrets.token_hex(32)
    return dict(bearer=bearer, admin_bearer=admin_bearer, outsider_bearer=outsider_bearer,
                refresh_token=secrets.token_hex(32), client_id='native-am-local-client', client_secret=secrets.token_hex(32),
                expires_at=int(time.time()) + 3600, control_key=secrets.token_hex(32),
                mongo_root=secrets.token_hex(24), mongo_app=secrets.token_hex(24),
                replica_key=base64.b64encode(secrets.token_bytes(48)).decode(),
                encryption_key=base64.b64encode(secrets.token_bytes(32)).decode())


def dependencies(secret):
    config = {'apiVersion': 'v1', 'kind': 'ConfigMap', 'metadata': metadata('client-code'),
              'data': {'identity.py': Path(__file__).with_name('identity.py').read_text()}}
    protected = {'apiVersion': 'v1', 'kind': 'Secret', 'metadata': metadata('client-secrets'),
                'stringData': {'identity.json': json.dumps({
                    **{k: secret[k] for k in ['bearer', 'admin_bearer', 'outsider_bearer',
                       'refresh_token', 'client_id', 'client_secret', 'expires_at']},
                    'deployment_id': DEPLOYMENT}),
                    'control-key': secret['control_key'], 'replica-key': secret['replica_key'],
                    'mongo-root': secret['mongo_root'], 'mongo-app': secret['mongo_app']}}
    mongo = pod('mongo', MONGO, resource('1', '3Gi', '250m', '2Gi'),
                command=['/bin/sh', '-ec'], args=[
                    'umask 077; cp /private/replica-key /tmp/replica-key; '
                    'chmod 600 /tmp/replica-key; chown mongodb:mongodb /tmp/replica-key; '
                    'exec docker-entrypoint.sh mongod --replSet nativeam '
                    '--keyFile /tmp/replica-key --bind_ip_all --wiredTigerCacheSizeGB 0.25 --oplogSize 64'],
                env=[{'name': 'MONGO_INITDB_ROOT_USERNAME', 'value': 'root'},
                     {'name': 'MONGO_INITDB_ROOT_PASSWORD', 'valueFrom': {'secretKeyRef':
                        {'name': 'client-secrets', 'key': 'mongo-root'}}}],
                volumeMounts=[{'name': 'private', 'mountPath': '/private', 'readOnly': True},
                              {'name': 'data', 'mountPath': '/data/db'}])
    mongo['spec']['volumes'] = [{'name': 'private', 'secret': {'secretName': 'client-secrets'}},
                                {'name': 'data', 'emptyDir': {'sizeLimit': '2Gi'}}]
    identity = pod('identity', BASELINE, resource('250m', '128Mi', '25m', '32Mi'),
                   command=['/usr/bin/python3', '-B', '/code/identity.py'],
                   volumeMounts=[{'name': 'private', 'mountPath': '/private', 'readOnly': True},
                                 {'name': 'code', 'mountPath': '/code', 'readOnly': True}])
    identity['spec']['volumes'] = [{'name': 'private', 'secret': {'secretName': 'client-secrets'}},
                                   {'name': 'code', 'configMap': {'name': 'client-code'}}]
    role = pod('am-role', AM, resource('250m', '256Mi', '25m', '32Mi'),
               args=['--print-runtime-role'], env=[{'name': 'CHRONO_MONGODB_DATABASE', 'value': 'cma_native_am'}])
    return [config, protected, service('mongo', 27017), service('identity', 8080), mongo, identity, role]


def am(secret, runtime, family='base', base_image=None):
    values = {
        'CHRONO_SERVER_BIND': '0.0.0.0:8080', 'CHRONO_METRICS_BIND': '0.0.0.0:9091',
        'CHRONO_SERVER_PUBLIC_URL': 'https://native-am.local.test',
        'CHRONO_SERVER_CORS_ORIGIN': 'https://native-am.local.test',
        'CHRONO_AUTH_MODE': 'provider', 'CHRONO_AUTH_ENCRYPTION_KEY': secret['encryption_key'],
        'CHRONO_NYXID_BASE_URL': 'http://identity:8080',
        'CHRONO_NYXID_CLIENT_ID': secret['client_id'], 'CHRONO_NYXID_CLIENT_SECRET': secret['client_secret'],
        'CHRONO_NYXID_PUBLIC_URL': 'https://identity.local.test',
        'CHRONO_MONGODB_URI': 'mongodb://nativeam:' + secret['mongo_app'] +
            '@mongo:27017/cma_native_am?authSource=cma_native_am&replicaSet=nativeam&directConnection=true',
        'CHRONO_MONGODB_DATABASE': 'cma_native_am', 'CHRONO_MONGODB_AUTHORITY_MODE': 'isolated',
        'CHRONO_DEPLOYMENT_ID': DEPLOYMENT, 'CHRONO_AGENT_MANAGER_REPLICAS_ENABLED': 'false',
        'CHRONO_SANDBOX_PROVIDER': 'agent-sandbox', 'CHRONO_RETAIN_OPENSANDBOX': 'false',
        'CHRONO_SANDBOX_IMAGE': runtime, 'CHRONO_SANDBOX_NAMESPACE': NS,
        'CHRONO_AGENT_SANDBOX_ENABLED': 'true', 'CHRONO_AGENT_SANDBOX_CONTROL_KEY_FILE': '/private/control-key',
        'CHRONO_AGENT_SANDBOX_RUNTIME_TRANSPORT': 'kubernetes-exec',
        'CHRONO_AGENT_SANDBOX_HOME_MODE': 'native_vct_full_home_v2',
        'CHRONO_AGENT_SANDBOX_HOME_STORAGE_CLASS': 'cma-native-rwop',
        'CHRONO_AGENT_SANDBOX_HOME_CAPACITY_GIB': '4',
        'CHRONO_AGENT_SANDBOX_SERVICE_ACCOUNT': 'cma-runtime',
        'CHRONO_NATIVE_PREPARED_TARGET': '0', 'CHRONO_NATIVE_MACHINE_PREPARED_TARGET': '0',
        'CHRONO_NATIVE_WARM_POOL_TARGET': '0', 'CHRONO_NATIVE_MACHINE_WARM_POOL_TARGET': '0',
        'CHRONO_AGENT_MANAGER_SANDBOX_CPU': '1', 'CHRONO_AGENT_MANAGER_SANDBOX_MEMORY': '2Gi',
        'CHRONO_SANDBOX_CPU': '1', 'CHRONO_SANDBOX_MEMORY': '2Gi',
        'CHRONO_UPSTREAM_BASE_URL': 'https://provider.native-am.invalid', 'CHRONO_UPSTREAM_WIRE_API': 'responses',
        'CHRONO_PLATFORM_BASE_URL': 'https://identity.native-am.invalid',
        'RUST_LOG': ('info,cma::sandbox=debug,layer3_adapter::agent_sandbox=debug,'
                     'cma::agent_manager::peer_write=debug,layer2_core::sandbox::settings_dispatch=debug'),
    }
    if family == 'omarchy':
        if not base_image or base_image == runtime:
            raise RuntimeError('machine_base_image_not_distinct')
        values.update(CHRONO_SANDBOX_IMAGE=base_image, CHRONO_MACHINE_SANDBOX_IMAGE=runtime,
            CHRONO_AGENT_MANAGER_SANDBOX_CPU='2', CHRONO_AGENT_MANAGER_SANDBOX_MEMORY='8Gi',
            CHRONO_SANDBOX_CPU='2', CHRONO_SANDBOX_MEMORY='8Gi')
    env = {'apiVersion': 'v1', 'kind': 'Secret', 'metadata': metadata('am-environment'), 'stringData': values}
    manager = pod('am', AM, resource('1', '1536Mi', '500m', '768Mi'),
                  envFrom=[{'secretRef': {'name': 'am-environment'}}],
                  volumeMounts=[{'name': 'private', 'mountPath': '/private', 'readOnly': True}],
                  readinessProbe={'httpGet': {'path': '/ready', 'port': 8080}, 'periodSeconds': 2})
    manager['spec'].update(serviceAccountName='cma-native-am', automountServiceAccountToken=True,
                           volumes=[{'name': 'private', 'secret': {'secretName': 'client-secrets'}}])
    return [env, service('am', 8080), manager]


MONGO_READY = Path(__file__).with_name('mongo_ready.js').read_text()


def mongo_role_script(role):
    return r"""
const admin=db.getSiblingDB('admin');
if(!admin.auth(process.env.MONGO_INITDB_ROOT_USERNAME,process.env.MONGO_INITDB_ROOT_PASSWORD))quit(2);
const target=db.getSiblingDB('cma_native_am');
const role=ROLE;
if(target.runCommand(role).ok!==1)throw new Error('role_failed');
if(target.runCommand({createUser:'nativeam',pwd:require('fs').readFileSync('/private/mongo-app','utf8'),
roles:[{role:role.createRole,db:'cma_native_am'}]}).ok!==1)throw new Error('user_failed');
if(target.sandboxes.countDocuments({})!==0)throw new Error('authority_not_empty');
print('ROLE_CREATED_AUTHORITY_EMPTY');
""".replace('ROLE', json.dumps(role, separators=(',', ':')))
