#!/usr/bin/env bash
set -euo pipefail
scale_root=$(cd "$(dirname "$0")/.." && pwd)
cd "$scale_root"
case "${1:-}" in
  fixed-1|fixed-2)
    replicas=${1#fixed-}
    exec python3 - "$scale_root" "$replicas" <<'PY'
import datetime
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time

root = Path(sys.argv[1])
n = int(sys.argv[2])
names = ['cma-agent-manager', 'cma-event-gateway']
scope = {'context': 'kind-cma-scale', 'namespace': 'cma-scale',
         'server': 'https://127.0.0.1:16443'}
os.umask(0o077)
parent = root / 'evidence/mode-transitions'
parent.mkdir(parents=True, exist_ok=True)
parent.resolve().relative_to(root.resolve())
receipt = Path(tempfile.mkdtemp(prefix='fixed-' + str(n) + '-', dir=parent))

def now():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()

def digest(value):
    return hashlib.sha256(value).hexdigest()

def save(name, value):
    # A new attempt owns a new directory; checkpoints are never replaced.
    with (receipt / name).open('x') as f:
        json.dump(value, f, indent=2)
        f.write('\n')

def event(value):
    with (receipt / 'events.jsonl').open('a') as f:
        f.write(json.dumps({'at_utc': now(), **value}) + '\n')

class CommandFailed(Exception):
    def __init__(self, code):
        self.code = code

phase = 'initialization'
status = 'failed'
exit_code = 1
error_type = None
save('intent.json', {'schema_version': 1, 'started_at_utc': now(), **scope,
     'mode': 'fixed-' + str(n), 'argv': ['scripts/mode.sh', 'fixed-' + str(n)],
     'control_sha256': {name: digest((root / 'scripts' / name).read_bytes())
                        for name in ['mode.sh', 'k']}})

def run(args, timeout=60):
    event({'phase': phase, 'state': 'started', 'argv': ['scripts/k', *args]})
    try:
        result = subprocess.run([str(root / 'scripts/k'), *args],
                                capture_output=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        event({'phase': phase, 'state': 'timeout', 'exit_code': 124})
        raise CommandFailed(124)
    # Retain command outcome, not unrestricted Kubernetes output or errors.
    event({'phase': phase, 'state': 'finished', 'exit_code': result.returncode,
           'stdout_sha256': digest(result.stdout), 'stderr_sha256': digest(result.stderr)})
    if result.returncode:
        raise CommandFailed(result.returncode)
    return result.stdout

def identity(obj):
    meta = obj['metadata']
    name = meta['name']
    if meta.get('namespace') != scope['namespace'] or not re.fullmatch(r'[a-z0-9]([-a-z0-9.]*[a-z0-9])?', name):
        raise ValueError('Unexpected resource identity')
    return {'name': name, 'namespace': meta['namespace'],
            'uid_sha256': digest(meta['uid'].encode()),
            'resourceVersion': meta['resourceVersion'], 'generation': meta.get('generation')}

try:
    phase = 'snapshot-hpa'
    hpas = json.loads(run(['get', 'hpa', '-o', 'json']))['items']
    selected = []
    for hpa in hpas:
        target = hpa['spec']['scaleTargetRef']
        if target.get('apiVersion') == 'apps/v1' and target.get('kind') == 'Deployment' and target.get('name') in names:
            selected.append({'apiVersion': hpa['apiVersion'], 'kind': 'HorizontalPodAutoscaler',
                             'metadata': identity(hpa), 'spec': hpa['spec']})
    save('hpa-before.json', {**scope, 'captured_at_utc': now(), 'items': selected,
                            'unrelated_count': len(hpas) - len(selected)})
    phase = 'snapshot-deployments'
    deployments = []
    for name in names:
        obj = json.loads(run(['get', 'deployment', name, '-o', 'json']))
        meta = identity(obj)
        if meta['name'] != name:
            raise ValueError('Unexpected deployment')
        template = obj['spec']['template']
        deployments.append({'metadata': meta, 'replicas': obj['spec']['replicas'],
            'source_sha': template['metadata'].get('annotations', {}).get('cma.chronoai.dev/source-sha'),
            'images': [c['image'] for c in template['spec']['containers']]})
    save('deployments-before.json', {**scope, 'items': deployments})
    # No mutation starts until all initial snapshots have succeeded.
    for hpa in selected:
        phase = 'delete-hpa:' + hpa['metadata']['name']
        run(['delete', 'hpa', hpa['metadata']['name'], '--wait=false'])
    for name in names:
        phase = 'scale:' + name
        run(['scale', 'deployment/' + name, '--replicas=' + str(n)])
    for name in names:
        phase = 'rollout:' + name
        run(['rollout', 'status', 'deployment/' + name, '--timeout=900s'], timeout=930)
    phase = 'verify-exact-pods'
    deadline = time.monotonic() + 900
    while time.monotonic() < deadline:
        good = True
        observed = {}
        for name in names:
            pods = json.loads(run(['get', 'pods', '-l', 'app.kubernetes.io/name=' + name, '-o', 'json']))['items']
            good &= len(pods) == n and all(not p['metadata'].get('deletionTimestamp') and
                any(c['type'] == 'Ready' and c['status'] == 'True' for c in p['status'].get('conditions', [])) for p in pods)
            observed[name] = [{'pod_uid_sha256': digest(p['metadata']['uid'].encode()),
                'node_sha256': digest(p['spec']['nodeName'].encode()),
                'terminating': bool(p['metadata'].get('deletionTimestamp')),
                'image_ids': [c.get('imageID') for c in p['status'].get('containerStatuses', [])]}
                for p in pods]
        if good:
            save('pods-after.json', {**scope, 'replicas': n, 'observed_at_utc': now(), 'services': observed})
            break
        time.sleep(2)
    else:
        raise TimeoutError('Exact serving replica count was not reached')
    phase = 'complete'
    status = 'succeeded'
    exit_code = 0
except CommandFailed as exc:
    exit_code = exc.code if 0 < exc.code < 256 else 1
    error_type = 'CommandFailed'
except KeyboardInterrupt:
    status, exit_code, error_type = 'interrupted', 130, 'KeyboardInterrupt'
except Exception as exc:
    error_type = type(exc).__name__
finally:
    save('result.json', {'finished_at_utc': now(), 'status': status, 'phase': phase,
                         'exit_code': exit_code, 'error_type': error_type})
    print(json.dumps({'status': status, 'mode': 'fixed-' + str(n), 'receipt': str(receipt),
                      'phase': phase, 'exit_code': exit_code}))
sys.exit(exit_code)
PY
    ;;
  auto)
    test -f manifests/hpa-local.json || { echo 'BLOCKED: local HPA manifest is missing' >&2; exit 1; }
    python3 - <<'PY'
import json
from pathlib import Path
items=json.loads(Path('manifests/hpa-local.json').read_text())['items']
names={'cma-agent-manager','cma-event-gateway'}
if len(items)!=2 or {obj['metadata']['name'] for obj in items}!=names:
    raise SystemExit('Refused unexpected local HPA set')
for obj in items:
    if obj['metadata'].get('namespace')!='cma-scale': raise SystemExit('Refused non-local namespace')
    if obj['apiVersion']!='autoscaling/v2' or obj['kind']!='HorizontalPodAutoscaler':
        raise SystemExit('Refused unexpected manifest kind')
    if obj['spec']['scaleTargetRef']!={'apiVersion':'apps/v1','kind':'Deployment','name':obj['metadata']['name']}:
        raise SystemExit('Refused unrelated scale target')
    if not (obj['spec']['minReplicas']==2 and obj['spec']['maxReplicas']==4):
        raise SystemExit('Refused HPA outside the local resource budget')
PY
    scripts/k apply --dry-run=server -f manifests/hpa-local.json >/dev/null
    scripts/k apply -f manifests/hpa-local.json
    ;;
  *) echo 'Usage: mode.sh fixed-1|fixed-2|auto' >&2; exit 2 ;;
esac
scripts/k get deployments,hpa,pdb
