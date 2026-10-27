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
for attempt in $(seq 1 60); do
  if docker exec "$contract_name" mongosh --quiet admin --eval \
    'db.auth(process.env.MONGO_INITDB_ROOT_USERNAME, process.env.MONGO_INITDB_ROOT_PASSWORD); if (db.runCommand({connectionStatus:1}).authInfo.authenticatedUsers.length !== 1) quit(1)' \
      >/dev/null 2>&1; then
    break
  fi
  [[ "$attempt" -lt 60 ]] || exit 1
  sleep 1
done
docker exec "$contract_name" mongosh --quiet admin --eval \
  'db.auth(process.env.MONGO_INITDB_ROOT_USERNAME, process.env.MONGO_INITDB_ROOT_PASSWORD); rs.initiate({_id:"cma-im-contract",members:[{_id:0,host:"127.0.0.1:27017"}]}); while (!db.hello().isWritablePrimary) sleep(200)' >/dev/null
endpoint=$(docker port "$contract_name" 27017/tcp)
[[ "$endpoint" =~ ^127\.0\.0\.1:[0-9]+$ ]] || exit 1
export CMA_IM_TEST_MONGODB_URI="mongodb://${MONGO_INITDB_ROOT_USERNAME}:${MONGO_INITDB_ROOT_PASSWORD}@${endpoint}/admin?directConnection=true"
cd "$repo_root/api"
cargo build --locked -p cmaeg
cargo test --locked -p cma-image-manager --test mongo_isolation --test browser_session \
  -- --ignored --test-threads=1

cargo test --locked -p cmaeg --lib optional_forms_storage -- --ignored --test-threads=1
cargo test --locked -p cmaeg --test mongo_isolation -- --ignored --test-threads=1
cargo test --locked -p cma store::isolation::tests::real_ -- --ignored --test-threads=1

cargo test --locked -p cmaeg --test management binary::compiled_binary_ -- --include-ignored --test-threads=1

cargo test --locked -p cmaeg --test relay_ingress runner_binary:: -- --ignored --test-threads=1

cargo test --locked -p cma gateway::tests:: -- --ignored --test-threads=1
