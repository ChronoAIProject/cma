#!/usr/bin/env bash
set -euo pipefail
scale_root=$(cd "$(dirname "$0")/.." && pwd)
cd "$scale_root"
case "${2:-}" in
  am) workload=cma-agent-manager ;;
  eg) workload=cma-event-gateway ;;
  *) echo 'Usage: fault.sh kill am|eg | fault.sh delay am|eg MILLISECONDS SECONDS' >&2; exit 2 ;;
esac
case "${1:-}" in
  kill)
    ready=$(scripts/k get deployment "$workload" -o jsonpath='{.status.readyReplicas}')
    [[ "$ready" =~ ^[0-9]+$ && "$ready" -ge 2 ]] || { echo 'Require at least two Ready replicas for a sudden-loss drill' >&2; exit 2; }
    pod=$(scripts/k get pods -l "app.kubernetes.io/name=$workload" --field-selector=status.phase=Running -o jsonpath='{.items[0].metadata.name}')
    printf '%s\n' "$pod" | shasum -a 256
    scripts/k delete pod "$pod" --grace-period=0 --force --wait=false >/dev/null
    scripts/k rollout status deployment/"$workload" --timeout=180s
    ;;
  delay)
    [[ "${3:-}" =~ ^[0-9]+$ && "${4:-}" =~ ^[0-9]+$ ]] || exit 2
    (( $3 >= 1 && $3 <= 5000 && $4 >= 1 && $4 <= 300 )) || exit 2
    scripts/k port-forward --address 127.0.0.1 service/toxiproxy 18474:8474 > private/toxiproxy-forward.log 2>&1 &
    forward_pid=$!
    cleanup() {
      curl --silent --output /dev/null --request DELETE "http://127.0.0.1:18474/proxies/$2/toxics/cma-scale-latency" || true
      kill "$forward_pid" 2>/dev/null || true
      wait "$forward_pid" 2>/dev/null || true
    }
    trap 'cleanup "$@"' EXIT
    trap 'exit 130' INT TERM
    for attempt in {1..30}; do
      kill -0 "$forward_pid" 2>/dev/null || { echo 'Owned port forward exited' >&2; exit 1; }
      if curl --fail --silent http://127.0.0.1:18474/version >/dev/null; then break; fi
      sleep 0.2
    done
    curl --fail --silent --output /dev/null -H 'Content-Type: application/json' \
      --data "{\"name\":\"cma-scale-latency\",\"type\":\"latency\",\"stream\":\"downstream\",\"toxicity\":1,\"attributes\":{\"latency\":$3,\"jitter\":0}}" \
      "http://127.0.0.1:18474/proxies/$2/toxics"
    echo "Delay active for $4 seconds; select it with X-Scale-Fault: delay"
    sleep "$4"
    ;;
  *) exit 2 ;;
esac
