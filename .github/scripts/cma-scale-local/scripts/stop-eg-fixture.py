#!/usr/bin/env python3
"""Owner-operated gateway finality after inner traffic, before fixture reconciliation."""
import argparse
import datetime
import json
import os
from pathlib import Path
import re
import subprocess

parser = argparse.ArgumentParser()
parser.add_argument('--run-name', required=True)
parser.add_argument('--source', required=True)
parser.add_argument('--count', type=int, choices=[1, 2], required=True)
args = parser.parse_args()
if not re.fullmatch('[a-z0-9][a-z0-9-]{0,31}', args.run_name):
    parser.error('invalid run name')
if not re.fullmatch('[0-9a-f]{40}', args.source):
    parser.error('expected a full source SHA')
root = Path(__file__).resolve().parent.parent
private = root / ('private/eg-fixture-' + args.run_name)
record = private / 'gateway-finality.json'
fd = os.open(record, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
k = ['kubectl', '--kubeconfig', str(root / 'private/kubeconfig'),
     '--context', 'kind-cma-scale', '--server', 'https://127.0.0.1:16443',
     '--namespace', 'cma-scale']
result = {'source': args.source, 'started_at_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
          'complete': False, 'containers': []}
try:
    d = json.loads(subprocess.check_output(k + ['get', 'deployment', 'cma-event-gateway', '-o', 'json'], text=True))
    if d['spec']['replicas'] != args.count or d['spec']['template']['metadata']['annotations'].get('cma.chronoai.dev/source-sha') != args.source:
        raise RuntimeError('Gateway source or count changed')
    app = next(c for c in d['spec']['template']['spec']['containers'] if c['name'] == 'cmaeg')
    if not any(e.get('valueFrom', {}).get('secretKeyRef', {}).get('name') == 'eg-diagnostic-' + args.run_name for e in app.get('env', [])):
        raise RuntimeError('Gateway is not bound to the selected fixture')
    hpas = json.loads(subprocess.check_output(k + ['get', 'hpa', '-o', 'json'], text=True))['items']
    if any(h['spec']['scaleTargetRef'].get('name') == 'cma-event-gateway' for h in hpas):
        raise RuntimeError('An active gateway HPA prevents finality')
    pods = json.loads(subprocess.check_output(k + ['get', 'pods', '-l',
        'app.kubernetes.io/name=cma-event-gateway', '-o', 'json'], text=True))['items']
    if len(pods) != args.count or any(p['metadata'].get('deletionTimestamp') for p in pods):
        raise RuntimeError('Unexpected or already terminating gateway Pods')
    for p in pods:
        if p['spec']['nodeName'] not in ('cma-scale-worker', 'cma-scale-worker2'):
            raise RuntimeError('Unexpected kind worker')
        for c in p['status']['containerStatuses']:
            if not c['containerID'].startswith('containerd://'):
                raise RuntimeError('Unexpected container runtime')
            result['containers'].append({'pod': p['metadata']['name'], 'pod_uid': p['metadata']['uid'],
                'node': p['spec']['nodeName'], 'container': c['name'],
                'id': c['containerID'].split('://', 1)[1]})
    subprocess.run(k + ['scale', 'deployment/cma-event-gateway', '--replicas=0',
        '--current-replicas=' + str(args.count), '--resource-version=' + d['metadata']['resourceVersion']], check=True)
    subprocess.run(k + ['wait', '--for=delete', *(('pod/' + p['metadata']['name']) for p in pods), '--timeout=840s'], check=True)
    after = json.loads(subprocess.check_output(k + ['get', 'pods', '-l',
        'app.kubernetes.io/name=cma-event-gateway', '-o', 'json'], text=True))['items']
    if after:
        raise RuntimeError('Gateway Pods still exist after shutdown')
    for node in {c['node'] for c in result['containers']}:
        # Read the kind node's container list only; no pod exec or environment inspection.
        containers = json.loads(subprocess.check_output(['docker', 'exec', node,
            'crictl', 'ps', '-a', '-o', 'json'], text=True))['containers']
        indexed = {c['id']: c['state'] for c in containers}
        for c in result['containers']:
            if c['node'] == node:
                c['final_state'] = indexed.get(c['id'], 'absent-from-runtime')
                if c['final_state'] not in ('CONTAINER_EXITED', 'absent-from-runtime'):
                    raise RuntimeError('A recorded gateway container has not exited')
    result['complete'] = True
    result['finished_at_utc'] = datetime.datetime.now(datetime.timezone.utc).isoformat()
    print('Gateway finality: zero Pods; all recorded application/proxy containers exited or removed')
    print('Private finality record:', record)
finally:
    with os.fdopen(fd, 'w') as output:
        json.dump(result, output, indent=2)
        output.write('\n')
