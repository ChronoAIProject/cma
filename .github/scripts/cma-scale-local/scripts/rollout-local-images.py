#!/usr/bin/env python3
"""Load an exact local build and replace only image/source fields at default two."""
import argparse
import json
from pathlib import Path
import re
import subprocess
import time

parser = argparse.ArgumentParser()
parser.add_argument('source_sha')
args = parser.parse_args()
if not re.fullmatch('[0-9a-f]{40}', args.source_sha):
    parser.error('expected a full Git SHA')
root = Path(__file__).resolve().parent.parent
k = ['kubectl', '--kubeconfig', str(root / 'private/kubeconfig'),
     '--context', 'kind-cma-scale', '--server', 'https://127.0.0.1:16443',
     '--namespace', 'cma-scale']
targets = [('cma-agent-manager', 'api'), ('cma-event-gateway', 'cmaeg')]
images = ['cma-scale-api:amd64-' + args.source_sha[:12],
          'cma-scale-eg:amd64-' + args.source_sha[:12]]
for image in images:
    data = json.loads(subprocess.check_output(['docker', 'image', 'inspect', image], text=True))[0]
    if data['Architecture'] != 'amd64' or data['Os'] != 'linux':
        raise SystemExit('Unexpected image platform: ' + image)
    # Source labels are supplied by the original Dockerfiles during packaging.
    if data['Config'].get('Labels', {}).get('org.opencontainers.image.revision') != args.source_sha:
        raise SystemExit('Image source label differs from requested commit: ' + image)
hpas = json.loads(subprocess.check_output(k + ['get', 'hpa', '-o', 'json'], text=True))
if any(h['spec']['scaleTargetRef'].get('name') in dict(targets) for h in hpas['items']):
    raise SystemExit('End the automatic cohort and select fixed-2 before image rollout')
for name, _ in targets:
    d = json.loads(subprocess.check_output(k + ['get', 'deployment', name, '-o', 'json'], text=True))
    if d['spec']['replicas'] != 2:
        raise SystemExit('End the fixed cohort and select fixed-2 before image rollout')
subprocess.run([str(root / 'bin/kind'), 'load', 'docker-image', '--name', 'cma-scale', *images], check=True)
for (name, container), image in zip(targets, images):
    patch = {'spec': {'template': {
        'metadata': {'annotations': {'cma.chronoai.dev/source-sha': args.source_sha}},
        'spec': {'containers': [{'name': container, 'image': image}]}}}}
    subprocess.run(k + ['patch', 'deployment', name, '--type=strategic', '-p', json.dumps(patch)], check=True)
subprocess.run(k + ['rollout', 'status', *(f'deployment/{n}' for n, _ in targets),
                    '--timeout=840s'], check=True)
# Deployment availability can succeed while old Pods still drain. Measurements
# must wait for their actual deletion, rather than treating rollout status as finality.
deadline = time.monotonic() + 840
while True:
    pods = json.loads(subprocess.check_output(k + ['get', 'pods', '-o', 'json'], text=True))['items']
    owned = [p for p in pods if p['metadata'].get('labels', {}).get('app.kubernetes.io/name') in dict(targets)]
    if len(owned) == 4 and not any(p['metadata'].get('deletionTimestamp') for p in owned):
        break
    if time.monotonic() >= deadline:
        raise SystemExit('Previous application Pods did not terminate within840s')
    time.sleep(5)
subprocess.run(['python3', str(root / 'scripts/observe-deployment.py'),
                args.source_sha, '--record'], check=True)
