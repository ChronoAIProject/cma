#!/usr/bin/env bash
set -euo pipefail
scale_root=$(cd "$(dirname "$0")/.." && pwd)
cd "$scale_root"
case "${1:-}" in
  fixed-1|fixed-2)
    replicas=${1#fixed-}
    scripts/k get hpa -o json > evidence/hpa-before-fixed.json
    python3 - <<'PY'
import json,subprocess
from pathlib import Path
for hpa in json.loads(Path('evidence/hpa-before-fixed.json').read_text())['items']:
    if hpa['spec']['scaleTargetRef'].get('name') in ['cma-agent-manager','cma-event-gateway']:
        subprocess.run(['scripts/k','delete','hpa',hpa['metadata']['name']],check=True)
PY
    scripts/k scale deployment/cma-agent-manager deployment/cma-event-gateway --replicas="$replicas"
    scripts/k rollout status deployment/cma-agent-manager --timeout=900s
    scripts/k rollout status deployment/cma-event-gateway --timeout=900s
    # Wait for old replicas to disappear so the N=1 cohort excludes draining pods.
    python3 - "$replicas" <<'PY'
import json,subprocess,sys,time
n=int(sys.argv[1]); deadline=time.monotonic()+900
while time.monotonic()<deadline:
    good=True
    for name in ['cma-agent-manager','cma-event-gateway']:
        pods=json.loads(subprocess.check_output(['scripts/k','get','pods','-l','app.kubernetes.io/name='+name,'-o','json']))['items']
        good &= len(pods)==n and all(not p['metadata'].get('deletionTimestamp') and
            any(c['type']=='Ready' and c['status']=='True' for c in p['status'].get('conditions',[])) for p in pods)
    if good: break
    time.sleep(2)
else: raise SystemExit('Timed out waiting for exact serving replica count')
PY
    ;;
  auto)
    test -f manifests/hpa-pdb-reviewed.json || { echo 'BLOCKED: developer-16 reviewed HPA/PDB handoff is required' >&2; exit 1; }
    python3 - <<'PY'
import json
from pathlib import Path
items=json.loads(Path('manifests/hpa-pdb-reviewed.json').read_text())['items']
for obj in items:
    if obj['metadata'].get('namespace')!='cma-scale': raise SystemExit('Refused non-local namespace')
    if obj['metadata'].get('annotations',{}).get('cma.chronoai.dev/review-state')!='approved':
        raise SystemExit('BLOCKED: exact-version review approval is absent')
    if obj['kind']=='HorizontalPodAutoscaler':
        if not (obj['spec']['minReplicas']==2 and obj['spec']['maxReplicas']<=4):
            raise SystemExit('Refused HPA outside the local resource budget')
    elif obj['kind']!='PodDisruptionBudget': raise SystemExit('Refused unexpected manifest kind')
PY
    scripts/k apply --dry-run=server -f manifests/hpa-pdb-reviewed.json >/dev/null
    scripts/k apply -f manifests/hpa-pdb-reviewed.json
    ;;
  *) echo 'Usage: mode.sh fixed-1|fixed-2|auto' >&2; exit 2 ;;
esac
scripts/k get deployments,hpa,pdb
