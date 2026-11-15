#!/usr/bin/env bash
set -euo pipefail
scale_root=$(cd "$(dirname "$0")/.." && pwd)
cd "$scale_root"
scripts/k get nodes >/dev/null
mkdir -p cache manifests
curl -fsSL https://github.com/kubernetes-sigs/metrics-server/releases/download/v0.8.0/components.yaml -o manifests/metrics-server-upstream.yaml
curl -fsSL https://github.com/kubernetes-sigs/agent-sandbox/releases/download/v1.0.2/sandbox-with-extensions.yaml -o cache/agent-sandbox-upstream.yaml
python3 - <<'PY'
from pathlib import Path
import hashlib
inputs={
 'manifests/metrics-server-upstream.yaml':'ff64d1a13b9ac3b0635f0dd985815fb44c23eed4706c04e5db1daadf6bc0a83b',
 'cache/agent-sandbox-upstream.yaml':'e3dda8359fee2b1bb6f4a9448d7004888286482b17670c581dee976f370fd165'}
for path,want in inputs.items():
 if hashlib.sha256(Path(path).read_bytes()).hexdigest()!=want: raise SystemExit('Upstream manifest checksum mismatch')
s=Path('manifests/metrics-server-upstream.yaml').read_text()
s=s.replace('        - --metric-resolution=15s','        - --metric-resolution=15s\n        - --kubelet-insecure-tls')
s=s.replace('        resources:\n          requests:', '        resources:\n          limits:\n            cpu: 200m\n            memory: 256Mi\n          requests:')
Path('manifests/metrics-server-local.yaml').write_text(s)
s=Path('cache/agent-sandbox-upstream.yaml').read_text()
s=s.replace('        name: agent-sandbox-controller\n        ports:', '        name: agent-sandbox-controller\n        resources:\n          requests:\n            cpu: 50m\n            memory: 64Mi\n          limits:\n            cpu: 250m\n            memory: 256Mi\n        ports:')
Path('manifests/agent-sandbox-local.yaml').write_text(s)
PY
kubectl --kubeconfig private/kubeconfig --context kind-cma-scale apply -f manifests/metrics-server-local.yaml
kubectl --kubeconfig private/kubeconfig --context kind-cma-scale apply --server-side --field-manager=cma-scale-local -f manifests/agent-sandbox-local.yaml
