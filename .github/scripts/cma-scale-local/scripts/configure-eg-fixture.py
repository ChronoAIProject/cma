#!/usr/bin/env python3
"""Load a prepared private EG cohort into the owned local Deployment."""
import argparse
import base64
import json
import re
from pathlib import Path
import subprocess

parser = argparse.ArgumentParser()
parser.add_argument('--cohort', choices=['off', 'on'], required=True)
parser.add_argument('--run-name')
args = parser.parse_args()
run_name = args.run_name or args.cohort
if not re.fullmatch('[a-z0-9][a-z0-9-]{0,31}', run_name):
    parser.error('run-name must be a short lowercase local resource name')
root = Path(__file__).resolve().parent.parent
private = root / ('private/eg-fixture-' + run_name)
bootstrap = json.loads((private / 'output/bootstrap.json').read_text())
config = json.loads((private / 'config.json').read_text())
if config['replicas_enabled'] != (args.cohort == 'on') or bootstrap['env']['CMAEG_REPLICAS_ENABLED'] != str(config['replicas_enabled']).lower():
    raise SystemExit('Fixture switch state does not match the selected cohort')
if len(base64.b64decode(bootstrap['cookie_key_base64'], validate=True)) != 32:
    raise SystemExit('Fixture cookie key must contain 32 bytes')
k = ['kubectl', '--kubeconfig', str(root / 'private/kubeconfig'),
     '--context', 'kind-cma-scale', '--server', 'https://127.0.0.1:16443',
     '--namespace', 'cma-scale']
secret_name = 'eg-diagnostic-' + run_name
secret = {'apiVersion': 'v1', 'kind': 'Secret',
          'metadata': {'name': secret_name, 'namespace': 'cma-scale'},
          'type': 'Opaque', 'stringData': bootstrap['env'],
          'data': {'cookie-key': bootstrap['cookie_key_base64']}}
subprocess.run(k + ['apply', '-f', '-'], input=json.dumps(secret), text=True,
               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True)
d = json.loads(subprocess.check_output(k + ['get', 'deployment', 'cma-event-gateway',
                                           '-o', 'json'], text=True))
spec = d['spec']['template']['spec']
app = next(c for c in spec['containers'] if c['name'] == 'cmaeg')
app['envFrom'] = [v for v in app.get('envFrom', [])
                  if not v.get('secretRef', {}).get('name', '').startswith('eg-diagnostic-')]
overrides = ['POD_IP', 'CMAEG_REPLICA_ADDRESS', 'CMAEG_COOKIE_KEY_FILE',
             'CMAEG_MONGODB_AUTHORITY_MODE']
app['env'] = [v for v in app.get('env', [])
              if v['name'] not in overrides and v['name'] not in bootstrap['env']]
# Select text fields explicitly: the same Secret also carries a binary cookie
# key for a file mount, which must never be expanded into the process environment.
app['env'] += [{'name': name, 'valueFrom': {'secretKeyRef': {
    'name': secret_name, 'key': name}}} for name in sorted(bootstrap['env'])]
app['env'] += [
    {'name': 'POD_IP', 'valueFrom': {'fieldRef': {'fieldPath': 'status.podIP'}}},
    {'name': 'CMAEG_REPLICA_ADDRESS', 'value': 'http://$(POD_IP):8090'},
    {'name': 'CMAEG_COOKIE_KEY_FILE', 'value': '/run/eg-diagnostic/cookie-key'},
    {'name': 'CMAEG_MONGODB_AUTHORITY_MODE', 'value': 'isolated'},
]
app['volumeMounts'] = [v for v in app.get('volumeMounts', []) if v['name'] != 'eg-diagnostic']
app['volumeMounts'].append({'name': 'eg-diagnostic', 'mountPath': '/run/eg-diagnostic',
                          'readOnly': True})
spec['volumes'] = [v for v in spec.get('volumes', []) if v['name'] != 'eg-diagnostic']
spec['volumes'].append({'name': 'eg-diagnostic', 'secret': {
    'secretName': secret_name, 'defaultMode': 0o440,
    'items': [{'key': 'cookie-key', 'path': 'cookie-key'}]}})
proxy = {
    'name': 'fixture-loopback',
    'image': 'alpine/socat@sha256:a6be4c0262b339c53ddad723cdd178a1a13271e1137c65e27f90a08c16de02b8',
    'args': ['TCP4-LISTEN:19100,bind=127.0.0.1,reuseaddr,fork',
             'TCP4:host.docker.internal:19100,connect-timeout=3'],
    'resources': {'requests': {'cpu': '5m', 'memory': '16Mi'},
                  'limits': {'cpu': '50m', 'memory': '32Mi'}},
    'securityContext': {'allowPrivilegeEscalation': False, 'readOnlyRootFilesystem': True,
                        'capabilities': {'drop': ['ALL']}},
}
spec['containers'] = [c for c in spec['containers'] if c['name'] != 'fixture-loopback'] + [proxy]
annotations = d['spec']['template']['metadata'].get('annotations', {})
annotations['cma.chronoai.dev/fixture-cohort'] = args.cohort
patch = [
    {'op': 'test', 'path': '/metadata/resourceVersion', 'value': d['metadata']['resourceVersion']},
    {'op': 'replace', 'path': '/spec/replicas', 'value': 1},
    {'op': 'replace', 'path': '/spec/template/spec', 'value': spec},
    {'op': 'add', 'path': '/spec/template/metadata/annotations', 'value': annotations},
]
path = private / 'deployment-patch.json'
path.write_text(json.dumps(patch, indent=2) + '\n')
path.chmod(0o600)
subprocess.run(k + ['patch', 'deployment', 'cma-event-gateway', '--type=json',
                    '--patch-file', str(path)], check=True)
print('Configured fresh', args.cohort, 'cohort at EG1 with loopback fixture proxy; wait for readiness before traffic.')
