#!/usr/bin/env bash
set -euo pipefail
scale_root=$(cd "$(dirname "$0")/.." && pwd)
cd "$scale_root"
mkdir -p private evidence
chmod 700 private
if ! bin/kind get clusters 2>/dev/null | grep -qx cma-scale; then
  bin/kind create cluster --name cma-scale --config manifests/kind.yaml \
    --kubeconfig "$scale_root/private/kubeconfig" \
    --image kindest/node:v1.34.0@sha256:7416a61b42b1662ca6ca89f02028ac133a309a2a30ba309614e8ec94d976dc5a --wait 180s
fi
chmod 600 private/kubeconfig
docker update --cpus 2 --memory 3g --memory-swap 3g cma-scale-control-plane
docker update --cpus 2 --memory 3584m --memory-swap 3584m cma-scale-worker cma-scale-worker2
scripts/k get nodes
