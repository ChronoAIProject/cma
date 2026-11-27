"""Two checkout-bound AM processes sharing one authenticated disposable authority."""
from copy import deepcopy
import json
import re
import secrets
from pathlib import Path
import legacy_manifests as old
from identity import ADMIN
from publication_evidence import FEATURES_MONGO_APP, PREPARED_MONGO_APP

API = None
SOURCE = None
ROOT = Path(__file__).resolve().parent

def configure(ready):
    global API,SOURCE
    image=ready['expected_api_image']
    source=ready.get('api_source',SOURCE)
    if not re.fullmatch(r'[^\s]+@sha256:[0-9a-f]{64}',image) or not re.fullmatch(r'[0-9a-f]{40}',source):
        raise ValueError('exact_api_identity_required')
    API=image
    SOURCE=source
    old.NS=ready['namespace']
    old.AM=API
    old.BASELINE=ready['runtime_images']['base']
    old.DEPLOYMENT=ready['run_id']
    if not re.fullmatch(r'localhost/multi-am-mongo@sha256:[0-9a-f]{64}', ready.get('mongo_image', '')):
        raise ValueError('exact_imported_mongo_required')
    old.MONGO=ready['mongo_image']

def dependencies(secret, ready):
    configure(ready)
    objects=old.dependencies(secret)
    role=next(o for o in objects if o['kind']=='Pod' and o['metadata']['name']=='am-role')
    if ready.get('fixture') in ('features', 'prepared'):
        mongo = next(o for o in objects if o['kind'] == 'Pod' and o['metadata']['name'] == 'mongo')
        mongo['spec']['containers'][0]['args'][0] += ' --setParameter enableTestCommands=1'
    # The exact image prints different least-privilege roles for OFF and ON.
    role['spec']['containers'][0]['env'].append(
        {'name':'CHRONO_AGENT_MANAGER_REPLICAS_ENABLED','value':'true'})
    code=next(o for o in objects if o['kind']=='ConfigMap')
    provider_path=Path(ready['provider_source'])
    code['data']['provider.py']=provider_path.read_text()
    code['data']['api_proxy.py']=(ROOT/'api_proxy.py').read_text()
    code['data']['storage.py']=(ROOT/'storage.py').read_text()
    private=next(o for o in objects if o['kind']=='Secret')
    private['stringData'].update({'provider-control':secret['provider_control'],
        'provider.crt':secret['provider_cert'],'provider.key':secret['provider_key']})
    if ready.get('expected_eg_image'):
        gateway_identity = {'client_id': 'disposable-gateway-fixture', 'client_secret': secrets.token_urlsafe(32)}
        secret['gateway_identity'] = gateway_identity
        identity = json.loads(private['stringData']['identity.json'])
        identity['gateway_client'] = gateway_identity
        private['stringData']['identity.json'] = json.dumps(identity)
        identity_service = next(o for o in objects if o['kind'] == 'Service' and o['metadata']['name'] == 'identity')
        identity_service['spec']['ports'][0]['name'] = 'http'
        identity_service['spec']['ports'].append({'name': 'https', 'port': 443, 'targetPort': 8443})
    private['stringData']['storage.json']=json.dumps({'client_id':secret['client_id'],
        'client_secret':secret['client_secret'],'access':secret['storage_access']})
    provider=old.pod('provider',old.BASELINE,old.resource('250m','256Mi','25m','32Mi'),
        command=['/usr/bin/python3','-B','/code/provider.py'],
        env=[{'name':'WS4_PROVIDER_CONTROL_TOKEN','valueFrom':{'secretKeyRef':
            {'name':'client-secrets','key':'provider-control'}}},
            {'name':'WS4_PROVIDER_TLS_CERT','value':'/private/provider.crt'},
            {'name':'WS4_PROVIDER_TLS_KEY','value':'/private/provider.key'},
            {'name':'PORT','value':'8080'},
            {'name':'WS4_PROVIDER_ALLOW_BOOTSTRAP','value':'true'}],
        volumeMounts=[{'name':'private','mountPath':'/private','readOnly':True},
                      {'name':'code','mountPath':'/code','readOnly':True}])
    provider['spec']['volumes']=[{'name':'private','secret':{'secretName':'client-secrets'}},
                                {'name':'code','configMap':{'name':'client-code'}}]
    objects.extend([old.service('provider',8080),provider])
    proxy=old.pod('api-proxy',old.BASELINE,old.resource('250m','128Mi','25m','32Mi'),
        command=['/usr/bin/python3','-B','/code/api_proxy.py'],
        volumeMounts=[{'name':'private','mountPath':'/private','readOnly':True},
                      {'name':'code','mountPath':'/code','readOnly':True}],
        readinessProbe={'tcpSocket':{'port':8443},'periodSeconds':2})
    proxy['spec']['volumes']=provider['spec']['volumes']
    service=old.service('api-proxy',443)
    service['spec']['ports'][0]['targetPort']=8443
    objects.extend([service,proxy])
    storage=old.pod('storage',old.BASELINE,old.resource('250m','256Mi','25m','64Mi'),
        command=['/usr/bin/python3','-B','/code/storage.py'],
        volumeMounts=deepcopy(proxy['spec']['containers'][0]['volumeMounts']),
        readinessProbe={'tcpSocket':{'port':8443},'periodSeconds':2})
    storage['spec']['volumes']=deepcopy(proxy['spec']['volumes'])
    service=old.service('storage',443)
    service['spec']['ports'][0]['targetPort']=8443
    objects.extend([service,storage])
    return objects

def managers(secret, ready):
    configure(ready)
    base=ready['runtime_images']['base']
    desktop=ready['runtime_images'].get('omarchy')
    template=old.am(secret,base)
    env=template[0]
    env['stringData'].update(CHRONO_AGENT_MANAGER_REPLICAS_ENABLED='true',
        CHRONO_SERVER_PUBLIC_URL='https://api.localhost.direct',
        CHRONO_SERVER_CORS_ORIGIN='https://api.localhost.direct',
        CHRONO_UPSTREAM_BASE_URL='https://provider.localhost.direct:8080/v1',
        CHRONO_PLATFORM_BASE_URL='https://storage.localhost.direct',
        CHRONO_DOCUMENT_BUCKET='cma-ci-documents',
        CHRONO_STORAGE_CLIENT_ID=secret['client_id'],
        CHRONO_STORAGE_CLIENT_SECRET=secret['client_secret'])
    # The native controller defaults a missing managed policy to public DNS.
    # Explicit fixture-only traffic preserves ClusterFirst for the two local
    # TLS routes, using the supported immutable native template configuration.
    env['stringData']['CHRONO_AGENT_SANDBOX_TEMPLATE_NETWORK_POLICY']=json.dumps({
        'ingress':[{'from':[{'podSelector':{}}]}],
        'egress':[{'to':[{'podSelector':{}}]},
            {'to':[{'namespaceSelector':{'matchLabels':{'kubernetes.io/metadata.name':'kube-system'}},
                    'podSelector':{'matchLabels':{'k8s-app':'kube-dns'}}}],
             'ports':[{'protocol':'UDP','port':53},{'protocol':'TCP','port':53}]}]},
        separators=(',',':'))
    env['stringData']['CHRONO_AGENT_CONTROL_WRITE_COORDINATION_ENABLED']='true'
    env['stringData'].update(CHRONO_USAGE_REPORT_VIEWS_ENABLED='true', CHRONO_USAGE_REPORT_WARMER_ENABLED='false', CHRONO_USAGE_ALLOCATION_ROLLUPS_ENABLED='false')
    env['stringData']['CHRONO_USAGE_REPORT_VIEW_SCOPES'] = json.dumps([
        {'principal': 'native-am-fixture-owner', 'admin': False, 'sandbox_id': None},
        {'principal': 'native-am-fixture-admin', 'admin': True, 'sandbox_id': None},
    ], separators=(',', ':'))
    prepared = ready.get('fixture') == 'prepared'
    if prepared:
        env['stringData']['CHRONO_NATIVE_PREPARED_TARGET'] = '1'
        env['stringData']['CHRONO_MONGODB_URI'] += '&appName=' + PREPARED_MONGO_APP
        # A prepared reservation remains scheduled during the cold comparison.
        # Both Standard runtimes must fit beside the two Managers and Kind.
        env['stringData']['CHRONO_AGENT_MANAGER_SANDBOX_CPU'] = '500m'
        env['stringData']['CHRONO_SANDBOX_CPU'] = '500m'
    elif ready.get('fixture') == 'features':
        env['stringData']['CHRONO_ADMISSION_CONTROL_WRITES_ENABLED'] = 'true'
        env['stringData']['CHRONO_ADMIN_RUNTIME_INVENTORY_ENABLED'] = 'true'
        env['stringData']['CHRONO_MONGODB_URI'] += '&appName=' + FEATURES_MONGO_APP
        env['stringData']['RUST_LOG'] += ',cma::agent_manager::writer_publication=debug,provenance_commit=debug'
    if desktop:
        env['stringData']['CHRONO_MACHINE_SANDBOX_IMAGE']=desktop
        env['stringData']['CHRONO_DESKTOP_VERIFIED_RUNTIME_IMAGE']=desktop
        capabilities = ['task_home_admission', 'desktop_enable']
        accounts = ['native-am-fixture-owner']
        if ready.get('fixture') == 'features':
            accounts.append(ADMIN)
        if not prepared:
            capabilities.append('suggested_followup_admission')
        env['stringData']['CHRONO_TASK_EXPERIENCE_POLICY']=json.dumps({
            'mode':'accounts','accounts':accounts,
            'capabilities':capabilities},separators=(',',':'))
        # This fixture retires its Standard home before starting the desktop.
        # Keep the one-CPU request and give either family enough memory for desktop.
        if not prepared:
            env['stringData']['CHRONO_AGENT_MANAGER_SANDBOX_MEMORY']='8Gi'
    public=old.service('am-public',8080)
    public['spec']['selector']={'ws4-public-am':'true'}
    result=[env,public]
    for suffix in ['a','b']:
        pod=deepcopy(template[2]); name='am-'+suffix
        pod['metadata']=old.metadata(name)
        pod['metadata']['labels']['ws4-public-am']='true'
        container=pod['spec']['containers'][0]
        container['name']=name
        container['env']=[{'name':'POD_IP','valueFrom':{'fieldRef':{'fieldPath':'status.podIP'}}},
            {'name':'CHRONO_AGENT_MANAGER_RELAY_ADDRESS','value':'http://$(POD_IP):8080'}]
        if ready.get('fixture') == 'features':
            pod['spec']['serviceAccountName'] = 'cma-native-am-' + suffix
        pod['spec']['restartPolicy'] = 'Always'
        stateful = {'apiVersion': 'apps/v1', 'kind': 'StatefulSet',
            'metadata': old.metadata(name), 'spec': {'serviceName': name, 'replicas': 1,
                'selector': {'matchLabels': {'app': name}},
                'template': {'metadata': {'labels': pod['metadata']['labels']}, 'spec': pod['spec']}}}
        result += [old.service(name,8080),stateful]
    return result
