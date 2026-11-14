#!/usr/bin/env bash
set -euo pipefail
scale_root=$(cd "$(dirname "$0")/.." && pwd)
cd "$scale_root"
label=${1:-smoke}
[[ "$label" =~ ^[a-z0-9_-]+$ ]] || exit 2
mkdir -p "evidence/$label"
docker run --rm --name cma-scale-load --network kind --cpus 0.5 --memory 256m --memory-swap 256m \
  --label dev.chrono.owner=local-env-manager-02 \
  -v "$scale_root/scripts:/scripts:ro" -v "$scale_root/evidence/$label:/results" \
  -v "$scale_root/private/tls.crt:/local-ca.crt:ro" -e SSL_CERT_FILE=/local-ca.crt \
  -e BASE_URL=https://cma-scale-control-plane:30080 -e DURATION="${DURATION:-30s}" -e FAULT="${FAULT:-}" \
  grafana/k6:1.3.0 run --quiet /scripts/smoke.js
echo "Infrastructure smoke summary: $scale_root/evidence/$label/summary.json"
