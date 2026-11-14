#!/usr/bin/env python3
"""Start private loopback forwards to the current fixed EG cohort."""
import argparse
import json
import os
from pathlib import Path
import re
import socket
import subprocess
import time
import urllib.request

parser = argparse.ArgumentParser()
parser.add_argument('--run-name', required=True)
parser.add_argument('--source', required=True)
parser.add_argument('--count', type=int, choices=[1, 2], required=True)
parser.add_argument('--base-port', type=int, default=28281)
args = parser.parse_args()
if not re.fullmatch('[a-z0-9][a-z0-9-]{0,31}', args.run_name):
    parser.error('invalid run name')
if not re.fullmatch('[0-9a-f]{40}', args.source):
    parser.error('expected a full source SHA')
if not 1024 <= args.base_port <= 65534:
    parser.error('base port must leave room for two unprivileged ports')
root = Path(__file__).resolve().parent.parent
private = root / ('private/eg-fixture-' + args.run_name)
if not (private / 'output/bootstrap.json').is_file():
    raise SystemExit('Prepared fixture bootstrap is missing')
record = private / ('endpoints-n' + str(args.count) + '.json')
if record.exists():
    raise SystemExit('Endpoint record already exists; retain it and use an explicit new attempt')
k = ['kubectl', '--kubeconfig', str(root / 'private/kubeconfig'),
     '--context', 'kind-cma-scale', '--server', 'https://127.0.0.1:16443',
     '--namespace', 'cma-scale']
pods = json.loads(subprocess.check_output(k + ['get', 'pods', '-l',
    'app.kubernetes.io/name=cma-event-gateway', '-o', 'json'], text=True))['items']
if len(pods) != args.count or any(p['metadata'].get('deletionTimestamp') or
    p['metadata'].get('annotations', {}).get('cma.chronoai.dev/source-sha') != args.source or
    not any(c['type'] == 'Ready' and c['status'] == 'True'
            for c in p['status'].get('conditions', [])) for p in pods):
    raise SystemExit('Expected exact ready nonterminating cohort at selected source')
processes, endpoints = [], []
try:
    for index, pod in enumerate(sorted(pods, key=lambda p: p['metadata']['name'])):
        port = args.base_port + index
        with socket.socket() as probe:
            probe.bind(('127.0.0.1', port))
        log = private / ('ingress-n' + str(args.count) + '-' + str(index) + '.log')
        fd = os.open(log, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, 'wb', buffering=0) as output:
            proc = subprocess.Popen(k + ['port-forward', '--address=127.0.0.1',
                'pod/' + pod['metadata']['name'], str(port) + ':8090'],
                stdout=output, stderr=output, start_new_session=True)
        processes.append(proc)
        endpoint = 'http://127.0.0.1:' + str(port)
        for attempt in range(50):
            if proc.poll() is not None:
                raise RuntimeError('Direct ingress forward exited')
            try:
                with urllib.request.urlopen(endpoint + '/readyz', timeout=1) as response:
                    if response.status == 200:
                        break
            except (OSError, TimeoutError):
                pass
            time.sleep(.1)
        else:
            raise RuntimeError('Direct ingress readiness did not reach200')
        endpoints.append({'endpoint': endpoint, 'pod': pod['metadata']['name'],
            'pod_uid': pod['metadata']['uid'], 'node': pod['spec']['nodeName'],
            'pod_ip': pod['status']['podIP'], 'pid': proc.pid, 'ready_http_status': 200,
            'source': args.source, 'images': [c['imageID']
                for c in pod['status']['containerStatuses'] if c['name'] == 'cmaeg']})
    fd = os.open(record, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, 'w') as output:
        json.dump({'endpoints': endpoints}, output, indent=2)
        output.write('\n')
except BaseException:
    for proc in processes:
        proc.terminate()
        proc.wait(timeout=10)
    raise
print('Direct ingress readiness200:', ', '.join(e['endpoint'] for e in endpoints))
print('Private Pod/process binding:', record)
