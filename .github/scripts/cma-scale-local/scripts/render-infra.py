#!/usr/bin/env python3
"""Render local infrastructure; credentials are generated once under private/."""
import base64
import json
import os
from pathlib import Path
import secrets

ROOT = Path(__file__).resolve().parent.parent
NS = 'cma-scale'

def resource(kind, name, spec=None, api='v1', **fields):
    obj = {'apiVersion': api, 'kind': kind, 'metadata': {'name': name, 'namespace': NS}, **fields}
    if spec is not None:
        obj['spec'] = spec
    return obj

def write(name, items, private=False):
    path = ROOT / ('private' if private else 'manifests') / name
    path.write_text(json.dumps({'apiVersion': 'v1', 'kind': 'List', 'items': items}, indent=2) + '\n')
    if private:
        path.chmod(0o600)

private = ROOT / 'private'
private.mkdir(exist_ok=True, mode=0o700)
credentials = private / 'credentials.json'
if not credentials.exists():
    credentials.write_text(json.dumps({
        'admin': secrets.token_hex(24), 'am': secrets.token_hex(24),
        'eg': secrets.token_hex(24), 'probe': secrets.token_hex(24),
        'keyfile': base64.b64encode(secrets.token_bytes(96)).decode(),
        'encryption': base64.b64encode(secrets.token_bytes(32)).decode(),
        'master': '1:' + base64.b64encode(secrets.token_bytes(32)).decode(),
        'service': secrets.token_hex(32), 'sandbox': secrets.token_hex(32),
    }))
    credentials.chmod(0o600)
creds = json.loads(credentials.read_text())
for purpose in ['am_identity', 'eg_identity']:
    if purpose not in creds:
        creds[purpose] = secrets.token_hex(32)
credentials.write_text(json.dumps(creds))
credentials.chmod(0o600)
hosts = ','.join(f'mongo-{i}.mongo.{NS}.svc.cluster.local:27017' for i in range(3))
def uri(user):
    return f'mongodb://{user}:{creds[user]}@{hosts}/cma_scale?replicaSet=rs0&authSource=cma_scale&maxPoolSize=10&serverSelectionTimeoutMS=5000'
write('secrets.json', [
    resource('Secret', 'mongo-bootstrap', type='Opaque', stringData={
        'ADMIN_PASSWORD': creds['admin'], 'AM_PASSWORD': creds['am'],
        'EG_PASSWORD': creds['eg'], 'PROBE_PASSWORD': creds['probe'], 'keyfile': creds['keyfile']}),
    resource('Secret', 'am-local', type='Opaque', stringData={
        'CHRONO_MONGODB_URI': uri('am'), 'CHRONO_AUTH_ENCRYPTION_KEY': creds['encryption'],
        'CHRONO_NYXID_CLIENT_SECRET': creds['am_identity'],
        'sandbox-key': creds['sandbox']}),
    resource('Secret', 'eg-local', type='Opaque', stringData={
        'CMAEG_MONGODB_URI': uri('eg'), 'CMAEG_MASTER_KEYS': creds['master'],
        'CMAEG_CMA_SERVICE_TOKEN': creds['service'], 'CMAEG_NYXID_CLIENT_SECRET': creds['eg_identity']}),
], private=True)

bootstrap = '''const admin = db.getSiblingDB('admin');
try { rs.status(); } catch (e) {
  if (e.code === 94) rs.initiate({_id:'rs0',members:[
    {_id:0,host:'mongo-0.mongo.cma-scale.svc.cluster.local:27017'},
    {_id:1,host:'mongo-1.mongo.cma-scale.svc.cluster.local:27017'},
    {_id:2,host:'mongo-2.mongo.cma-scale.svc.cluster.local:27017'}]});
}
for (let i=0; i<120; ++i) {
  if (admin.runCommand({hello:1}).isWritablePrimary) break;
  sleep(1000);
}
try { admin.createUser({user:'bootstrap',pwd:process.env.ADMIN_PASSWORD,roles:['root']}); }
catch (e) { if (![13,51003].includes(e.code)) throw e; }
admin.auth('bootstrap',process.env.ADMIN_PASSWORD);
const app=admin.getSiblingDB('cma_scale');
for (const [user,key] of [['am','AM_PASSWORD'],['eg','EG_PASSWORD']]) {
  if (!app.getUser(user)) app.createUser({user,pwd:process.env[key],roles:['readWrite','dbAdmin']});
}
if (!admin.getUser('probe')) admin.createUser({user:'probe',pwd:process.env.PROBE_PASSWORD,roles:['clusterMonitor']});
'''
entrypoint = '''#!/bin/bash
set -euo pipefail
mongod --replSet rs0 --bind_ip_all --keyFile /key/keyfile --auth \\
  --wiredTigerCacheSizeGB 0.25 --oplogSize 128 --quiet --logpath /data/db/mongod.log &
mongo_pid=$!
trap 'kill -TERM "$mongo_pid"; wait "$mongo_pid"' TERM INT
if [[ "$HOSTNAME" == mongo-0 && ! -f /data/db/.bootstrapped ]]; then
  for attempt in $(seq 1 120); do
    if mongosh --quiet --host 127.0.0.1 --eval 'quit(db.adminCommand({ping:1}).ok?0:1)' >/dev/null 2>&1; then break; fi
    sleep 1
  done
  if mongosh --quiet --host 127.0.0.1 /bootstrap/init.js >/dev/null 2>&1; then
    touch /data/db/.bootstrapped
  else
    echo 'Mongo bootstrap failed; inspect local state without exposing credentials' >&2
    kill -TERM "$mongo_pid"
    exit 1
  fi
fi
wait "$mongo_pid"
'''
mongo_security = {'runAsUser': 999, 'runAsGroup': 999, 'fsGroup': 999, 'seccompProfile': {'type': 'RuntimeDefault'}}
mongo = resource('StatefulSet', 'mongo', {
    'serviceName': 'mongo', 'replicas': 3, 'podManagementPolicy': 'Parallel',
    'selector': {'matchLabels': {'app': 'mongo'}},
    'template': {'metadata': {'labels': {'app': 'mongo'}}, 'spec': {
        'automountServiceAccountToken': False, 'securityContext': mongo_security,
        'terminationGracePeriodSeconds': 30,
        'topologySpreadConstraints': [{'maxSkew': 1, 'topologyKey': 'kubernetes.io/hostname',
            'whenUnsatisfiable': 'ScheduleAnyway', 'labelSelector': {'matchLabels': {'app': 'mongo'}}}],
        'initContainers': [{'name': 'key-permissions', 'image': 'mongo:8.0.13',
            'command': ['bash','-ec','cp /secret/keyfile /key/keyfile; chmod 400 /key/keyfile'],
            'resources': {'requests': {'cpu':'10m','memory':'16Mi'}, 'limits': {'cpu':'100m','memory':'64Mi'}},
            'volumeMounts': [{'name':'secret','mountPath':'/secret','readOnly':True}, {'name':'key','mountPath':'/key'}]}],
        'containers': [{'name':'mongo','image':'mongo:8.0.13', 'command':['bash','/bootstrap/start.sh'],
            'ports':[{'name':'mongo','containerPort':27017}],
            'envFrom':[{'secretRef':{'name':'mongo-bootstrap'}}],
            'resources': {'requests':{'cpu':'100m','memory':'384Mi'}, 'limits':{'cpu':'500m','memory':'768Mi'}},
            'securityContext': {'allowPrivilegeEscalation':False,'capabilities':{'drop':['ALL']}},
            'readinessProbe':{'exec':{'command':['mongosh','--quiet','--host','127.0.0.1','--eval',
                'const h=db.adminCommand({hello:1}); quit(h.isWritablePrimary||h.secondary?0:1)']},
                'periodSeconds':10,'timeoutSeconds':5,'failureThreshold':6},
            'livenessProbe':{'tcpSocket':{'port':27017},'initialDelaySeconds':120,'periodSeconds':20},
            'volumeMounts':[{'name':'data','mountPath':'/data/db'}, {'name':'key','mountPath':'/key','readOnly':True},
                {'name':'bootstrap','mountPath':'/bootstrap','readOnly':True}]}],
        'volumes':[{'name':'secret','secret':{'secretName':'mongo-bootstrap','defaultMode':288}},
            {'name':'key','emptyDir':{}}, {'name':'bootstrap','configMap':{'name':'mongo-bootstrap'}}],
    }},
    'volumeClaimTemplates':[{'metadata':{'name':'data'},'spec':{'accessModes':['ReadWriteOnce'],
        'resources':{'requests':{'storage':'3Gi'}}}}],
}, api='apps/v1')
namespace={'apiVersion':'v1','kind':'Namespace','metadata':{'name':NS,'labels':{'dev.chrono.owner':'local-env-manager-02'}}}
write('infra.json', [namespace,
    resource('ResourceQuota','local-budget',{'hard':{'requests.cpu':'4','requests.memory':'6Gi','limits.cpu':'9','limits.memory':'9Gi','requests.storage':'12Gi','pods':'24'}}),
    resource('Service','mongo',{'clusterIP':'None','publishNotReadyAddresses':True,'selector':{'app':'mongo'},'ports':[{'port':27017,'targetPort':27017}]}),
    resource('ConfigMap','mongo-bootstrap',data={'init.js':bootstrap,'start.sh':entrypoint}), mongo,
    resource('PodDisruptionBudget','mongo',{'minAvailable':2,'selector':{'matchLabels':{'app':'mongo'}}},api='policy/v1'),
])
print('Rendered infrastructure and private credentials; no credential values printed')
