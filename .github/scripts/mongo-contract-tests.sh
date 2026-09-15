#!/usr/bin/env bash
set -euo pipefail

# This entry point always creates its own disposable replica set. It does not
# accept a caller's Mongo URI, credentials, volume or existing container.
readonly mongo_image='mongo:8.0.13@sha256:cf340b1e5283843c63eb12999922f20c463ae31285f746d30f05dcc21cd1d47c'
contract_suffix=$(openssl rand -hex 12)
[[ "$contract_suffix" =~ ^[0-9a-f]{24}$ ]] || exit 1
readonly contract_name="cma-mongo-ci-${contract_suffix}"
readonly contract_label="dev.chrono.cma.mongo-contract=${contract_name}"
script_directory=$(dirname -- "${BASH_SOURCE[0]}")
repo_root=$(CDPATH='' cd -- "${script_directory}/../.." && pwd -P)
readonly repo_root
unset CHRONO_TEST_MONGODB_URI CHRONO_TEST_MONGODB_FAILPOINT_URI

cleanup() {
  local status=$? ids remaining id
  trap - EXIT INT TERM
  if ! ids=$(docker ps --all --quiet --filter "label=${contract_label}"); then
    exit 1
  fi
  for id in $ids; do
    [[ "$id" =~ ^[0-9a-f]{12,64}$ ]] || exit 1
    docker rm --force "$id" >/dev/null || status=1
  done
  remaining=$(docker ps --all --quiet --filter "label=${contract_label}") || exit 1
  [[ -z "$remaining" ]] || status=1
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

docker run --detach --name "$contract_name" --label "$contract_label" \
  --publish 127.0.0.1::27017 --memory 2g --cpus 2 --pids-limit 256 \
  --tmpfs /data/db:rw,size=1073741824 --tmpfs /data/configdb:rw,size=16777216 \
  "$mongo_image" --replSet cma_ci --bind_ip_all --setParameter enableTestCommands=1 >/dev/null
for attempt in $(seq 1 60); do
  if docker exec "$contract_name" mongosh --quiet --eval 'db.adminCommand({ping:1}).ok' >/dev/null 2>&1; then
    break
  fi
  [[ "$attempt" -lt 60 ]] || exit 1
  sleep 1
done
docker exec "$contract_name" mongosh --quiet --eval \
  'rs.initiate({_id:"cma_ci",members:[{_id:0,host:"localhost:27017"}]})' >/dev/null
for attempt in $(seq 1 60); do
  if docker exec "$contract_name" mongosh --quiet --eval \
    'if (!db.hello().isWritablePrimary) quit(1)' >/dev/null 2>&1; then
    break
  fi
  [[ "$attempt" -lt 60 ]] || exit 1
  sleep 1
done
endpoint=$(docker port "$contract_name" 27017/tcp)
[[ "$endpoint" =~ ^127\.0\.0\.1:[0-9]+$ ]] || exit 1
export CHRONO_TEST_MONGODB_URI="mongodb://${endpoint}/?directConnection=true&replicaSet=cma_ci"
export CHRONO_TEST_MONGODB_FAILPOINT_URI="$CHRONO_TEST_MONGODB_URI"
cd "$repo_root/api"
cargo nextest run --workspace --locked --run-ignored only --test-threads 1 --no-fail-fast \
  -E 'test(store::sandbox_mongo_integration_tests::warm_hierarchy_tests::) | test(store::agent_admission::mongo_integration_tests::) | test(store::schema_mongo_integration_tests::) | test(store::trigger_definitions::launch_mongo_integration_tests::) | test(store::checkpoint_objects::mongo_integration_tests::) | test(store::sandbox_mongo_integration_tests::checkpoint_capture_tests::) | test(store::sandbox_mongo_integration_tests::service_access_tests::) | test(store::sandbox_mongo_integration_tests::command_ledger_tests::) | test(store::sandbox_mongo_integration_tests::resource_access_tests::) | test(store::bridge_definitions::mongo_integration_tests::) | test(store::bridge_invokes::tests::) | test(store::bridge_invokes::outbox::tests::) | test(store::bridge_allocations::mongo_integration_tests::)'
