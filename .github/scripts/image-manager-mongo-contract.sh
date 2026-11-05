#!/usr/bin/env bash
set -euo pipefail

# Separate from the monolith's unauthenticated replica-set fixtures: these
# contracts must prove actual credential and collection authorization failures.
script_directory=$(dirname -- "${BASH_SOURCE[0]}")
repo_root=$(CDPATH='' cd -- "${script_directory}/../.." && pwd -P)
readonly mongo_image='mongo:8.0.13@sha256:cf340b1e5283843c63eb12999922f20c463ae31285f746d30f05dcc21cd1d47c'
contract_suffix=$(openssl rand -hex 12)
readonly contract_name="cma-im-mongo-${contract_suffix}"
readonly contract_label="dev.chrono.cma.im-contract=${contract_suffix}"
export MONGO_INITDB_ROOT_USERNAME=contract-admin
MONGO_INITDB_ROOT_PASSWORD=$(openssl rand -hex 24)
export MONGO_INITDB_ROOT_PASSWORD
CMA_IM_REPLICA_KEY=$(openssl rand -hex 64)
export CMA_IM_REPLICA_KEY

cleanup() {
  local status=$? owner
  trap - EXIT
  if docker container inspect "$contract_name" >/dev/null 2>&1; then
    owner=$(docker inspect --format '{{index .Config.Labels "dev.chrono.cma.im-contract"}}' "$contract_name")
    if [[ "$owner" != "$contract_suffix" ]]; then
      echo 'Refusing cleanup of a container without this fixture identity' >&2
      exit 1
    fi
    docker rm --force --volumes "$contract_name" >/dev/null || status=1
  fi
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

docker run --detach --name "$contract_name" --label "$contract_label" \
  --publish 127.0.0.1::27017 --memory 2g --memory-swap 2g --cpus 2 --pids-limit 256 \
  --tmpfs /data/db:rw,size=1073741824 --tmpfs /data/configdb:rw,size=16777216 \
  --env MONGO_INITDB_ROOT_USERNAME --env MONGO_INITDB_ROOT_PASSWORD --env CMA_IM_REPLICA_KEY \
  --entrypoint sh "$mongo_image" -c 'umask 077; printf "%s" "$CMA_IM_REPLICA_KEY" > /tmp/replica-key; chown mongodb:mongodb /tmp/replica-key; exec docker-entrypoint.sh mongod --replSet cma-im-contract --keyFile /tmp/replica-key --wiredTigerCacheSizeGB 0.25' >/dev/null
readiness_script=$(cat "$script_directory/authenticated-mongo-readiness.js")
for attempt in $(seq 1 60); do
  if docker exec "$contract_name" mongosh --quiet \
    'mongodb://127.0.0.1:27017/admin?serverSelectionTimeoutMS=1000&connectTimeoutMS=1000' \
    --eval "$readiness_script" \
      >/dev/null 2>&1; then
    break
  fi
  if [[ "$attempt" -eq 60 ]]; then
    echo 'Authenticated Mongo replica set did not become writable within the startup budget' >&2
    docker inspect --format 'State={{.State.Status}} OOMKilled={{.State.OOMKilled}} ExitCode={{.State.ExitCode}}' "$contract_name" >&2 || true
    exit 1
  fi
  sleep 1
done
endpoint=$(docker port "$contract_name" 27017/tcp)
[[ "$endpoint" =~ ^127\.0\.0\.1:[0-9]+$ ]] || exit 1
export CMA_IM_TEST_MONGODB_URI="mongodb://${MONGO_INITDB_ROOT_USERNAME}:${MONGO_INITDB_ROOT_PASSWORD}@${endpoint}/admin?directConnection=true"
cd "$repo_root/api"
cargo build --locked -p cmaeg
cargo test --locked -p cma-image-manager --test mongo_isolation --test browser_session \
  -- --ignored --test-threads=1

cargo test --locked -p cmaeg --lib optional_forms_storage -- --ignored --test-threads=1
cargo test --locked -p cmaeg --lib optional_media_continuation_storage -- --ignored --test-threads=1
# Owner-key custody needs both real authentication and a replica set. Reuse
# this owned fixture; exact nextest selection fails if the contract disappears.
cargo nextest run --workspace --locked \
  -E 'package(=cmaeg) and test(=worker_credentials::mongo::integration_tests::worker_history_cross_client_visibility_append_only_grants_and_cutover_barrier)' \
  --run-ignored only --test-threads 1
cargo test --locked -p cmaeg --lib whatsapp_retention_on_real_mongo -- --ignored --test-threads=1
cargo test --locked -p cmaeg --test mongo_isolation -- --ignored --test-threads=1
cargo test --locked -p cmaeg --test management github_integrations:: -- --ignored --test-threads=1
cargo test --locked -p cmaeg --test github_source -- --ignored --test-threads=1
cargo test --locked -p cmaeg --lib tools::threads::ledger::mongo_tests:: -- --ignored --test-threads=1
# Keep the workspace feature graph used by the earlier Backend tests, avoiding
# another aggregate CMA test link for a narrower package dependency graph.
cargo nextest run --workspace --locked -E 'package(=cma) and test(~store::isolation::tests::real_)' \
  --run-ignored only --test-threads 1

cargo test --locked -p cmaeg --test management binary::compiled_binary_ -- --include-ignored --test-threads=1

cargo test --locked -p cmaeg --test relay_ingress runner_binary:: -- --ignored --test-threads=1

cargo nextest run --workspace --locked -E 'package(=cma) and test(~gateway::tests::)' \
  --run-ignored only --test-threads 1
