#!/usr/bin/env bash
set -euo pipefail

# Separate from the monolith's unauthenticated replica-set fixtures: these
# contracts must prove actual credential and collection authorization failures.
script_directory=$(dirname -- "${BASH_SOURCE[0]}")
repo_root=$(CDPATH='' cd -- "${script_directory}/../.." && pwd -P)
unset CMA_IM_TEST_MONGODB_URI CHRONO_TEST_MONGODB_URI CHRONO_TEST_MONGODB_FAILPOINT_URI
# shellcheck source=.github/scripts/backend-nextest.sh
source "$script_directory/backend-nextest.sh"
# Select product groups before creating a fixture. The default preserves the
# full local contract for callers outside path-selected CI.
contract_track=${1:-all}
case "$contract_track" in am|eg|all) ;; *) echo 'expected am, eg or all' >&2; exit 2 ;; esac
contract_nextest() {
  local owner=$1
  shift
  if [[ "$contract_track" == all || "$contract_track" == "$owner" ]]; then
    nextest_build "$@"
  fi
}
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

bash "$repo_root/.github/scripts/pull-pinned-image.sh" "$mongo_image"
docker run --detach --name "$contract_name" --label "$contract_label" \
  --publish 127.0.0.1::27017 --memory 2g --memory-swap 2g --cpus 2 --pids-limit 256 \
  --tmpfs /data/db:rw,size=1073741824 --tmpfs /data/configdb:rw,size=16777216 \
  --env MONGO_INITDB_ROOT_USERNAME --env MONGO_INITDB_ROOT_PASSWORD --env CMA_IM_REPLICA_KEY \
  --entrypoint sh "$mongo_image" -c 'umask 077; printf "%s" "$CMA_IM_REPLICA_KEY" > /tmp/replica-key; chown mongodb:mongodb /tmp/replica-key; exec docker-entrypoint.sh mongod --replSet cma-im-contract --keyFile /tmp/replica-key --wiredTigerCacheSizeGB 0.25 --setParameter enableTestCommands=1' >/dev/null
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
# Shared Console authority uses this authenticated disposable replica set.
# Serial execution isolates its appName-scoped acknowledgement-loss failpoint.
contract_nextest eg run --run-ignored only --test-threads 1 --no-fail-fast \
  -E 'package(=cmaeg) and kind(lib) and test(~console::session_backend::)'
contract_nextest eg run --run-ignored only --test-threads 1 --no-fail-fast \
  -E 'package(=cmaeg) and kind(test) and binary(=console) and test(~shared_)'
contract_nextest am run --run-ignored only --test-threads 1 --no-fail-fast \
  -E 'package(=cma-image-manager) and kind(test) and (binary(=mongo_isolation) or binary(=browser_session))'

# Migration contracts require isolated credentials, real DDL and a replica set.
contract_nextest am run --run-ignored only --test-threads 1 --no-fail-fast \
  -E 'package(=cma-service-migrations) and kind(lib)'
contract_nextest am run --run-ignored only --test-threads 1 --no-fail-fast \
  -E 'package(=cma-image-manager) and kind(lib) and test(~migrations::tests::)'
contract_nextest eg run --run-ignored only --test-threads 1 --no-fail-fast \
  -E 'package(=cmaeg) and kind(lib) and test(~migrations::)'

contract_nextest eg run --run-ignored only --test-threads 1 --no-fail-fast \
  -E 'package(=cmaeg) and kind(lib) and test(~optional_forms_storage)'
contract_nextest eg run --run-ignored only --test-threads 1 --no-fail-fast \
  -E 'package(=cmaeg) and kind(lib) and test(~optional_media_continuation_storage)'
# A cancelled commit or abort must not strand its row locks, and the runtime
# grant must end its own session (#3299). Failpoints are server-global.
contract_nextest eg run --run-ignored only --test-threads 1 --no-fail-fast \
  -E 'package(=cmaeg) and kind(lib) and test(~transaction_terminal::)'
# History migration and writer grants require this authenticated replica set.
contract_nextest eg run --run-ignored only --test-threads 1 --no-fail-fast \
  -E 'package(=cmaeg) and kind(lib) and test(~activity::history::mongo_tests::)'
# Owner-key custody needs both real authentication and a replica set. Reuse
# this owned fixture; exact nextest selection fails if the contract disappears.
contract_nextest eg run \
  -E 'package(=cmaeg) and test(=worker_credentials::mongo::integration_tests::worker_history_cross_client_visibility_append_only_grants_and_cutover_barrier)' \
  --run-ignored only --test-threads 1
contract_nextest eg run --run-ignored only --test-threads 1 --no-fail-fast \
  -E 'package(=cmaeg) and kind(lib) and test(=channels::create_cleanup_tests::retirement_at_the_record_limit_preserves_tracked_worker_credentials)'
contract_nextest eg run --run-ignored only --test-threads 1 --no-fail-fast \
  -E 'package(=cmaeg) and kind(lib) and test(~whatsapp_retention_on_real_mongo)'
contract_nextest eg run --run-ignored only --test-threads 1 --no-fail-fast \
  -E 'package(=cmaeg) and kind(test) and binary(=mongo_isolation)'
contract_nextest eg run --run-ignored only --test-threads 1 --no-fail-fast \
  -E 'package(=cmaeg) and kind(test) and binary(=management) and test(~github_integrations::)'
contract_nextest eg run --run-ignored only --test-threads 1 --no-fail-fast \
  -E 'package(=cmaeg) and kind(test) and binary(=management) and test(~organization_channels::)'
contract_nextest eg run --run-ignored only --test-threads 1 --no-fail-fast \
  -E 'package(=cmaeg) and kind(test) and binary(=management) and test(=record_management::mongo_legacy_read_is_owned_and_read_only)'
contract_nextest eg run --run-ignored only --test-threads 1 --no-fail-fast \
  -E 'package(=cmaeg) and kind(test) and binary(=github_source)'
contract_nextest eg run --run-ignored only --test-threads 1 --no-fail-fast \
  -E 'package(=cmaeg) and kind(lib) and test(~tools::threads::ledger::mongo_tests::)'
contract_nextest am run -E 'package(=cma) and (test(~store::isolation::tests::real_) or test(~store::migrations::tests::segments_runtime::))' \
  --run-ignored only --test-threads 1

contract_nextest eg run --run-ignored all --test-threads 1 --no-fail-fast \
  -E 'package(=cmaeg) and kind(test) and binary(=management) and test(~binary::compiled_binary_)'

contract_nextest eg run --run-ignored only --test-threads 1 --no-fail-fast \
  -E 'package(=cmaeg) and kind(test) and binary(=relay_ingress) and test(~runner_binary::)'

contract_nextest eg run --run-ignored only --test-threads 1 --no-fail-fast \
  -E 'package(=cmaeg) and kind(test) and binary(=replica_admission_mongo)'

# Require corrupt-head isolation and healthy frontier progress against real Mongo.
contract_nextest eg run --run-ignored only --test-threads 1 --no-fail-fast \
  -E 'package(=cmaeg) and kind(lib) and test(~replicas::partition_discovery::)'

contract_nextest eg run --run-ignored only --test-threads 1 --no-fail-fast \
  -E 'package(=cmaeg) and kind(lib) and test(=records::mongo::replica_schema::tests::replica_migration_recovers_after_second_index_conflict)'

contract_nextest eg run --run-ignored only --test-threads 1 --no-fail-fast \
  -E 'package(=cmaeg) and kind(lib) and (test(~channels::create_fence_tests::) or test(~records::mongo::create_fences::tests::))'

# Explicit selection keeps replica delivery tests environmental in normal unit
# runs while requiring their real authenticated Mongo execution in this gate.
contract_nextest eg run --run-ignored only --test-threads 1 --no-fail-fast \
  -E 'package(=cmaeg) and kind(lib) and test(~turns::routed::replica_tool_tests::)'
contract_nextest eg run --run-ignored only --test-threads 1 --no-fail-fast \
  -E 'package(=cmaeg) and kind(lib) and test(~turns::routed::replica_legacy_tool_tests::)'
contract_nextest eg run --run-ignored only --test-threads 1 --no-fail-fast \
  -E 'package(=cmaeg) and kind(lib) and test(~turns::routed::compatibility_mongo_tests::)'
contract_nextest eg run --run-ignored only --test-threads 1 --no-fail-fast \
  -E 'package(=cmaeg) and kind(lib) and test(~turns::routed::tool_relay::mongo_tests::)'
contract_nextest eg run --run-ignored only --test-threads 1 --no-fail-fast \
  -E 'package(=cmaeg) and kind(lib) and test(~turns::routed::tool_effects::mongo_)'

contract_nextest eg run --run-ignored only --test-threads 1 --no-fail-fast \
  -E 'package(=cmaeg) and kind(lib) and test(~turns::routed::media_effects::tests::)'

# PR B's replica capability and unresolved-history regressions are outside the
# broad partition-discovery and delayed-effect selections above.
contract_nextest eg run --run-ignored only --test-threads 1 --no-fail-fast \
  -E 'package(=cmaeg) and kind(lib) and test(~replicas::membership::capability::)'
contract_nextest eg run --run-ignored only --test-threads 1 --no-fail-fast \
  -E 'package(=cmaeg) and kind(lib) and test(~replicas::unresolved::)'
contract_nextest eg run --run-ignored only --test-threads 1 --no-fail-fast \
  -E 'package(=cmaeg) and kind(lib) and test(~replicas::mongo_tests::)'

contract_nextest am run -E 'package(=cma) and test(~gateway::tests::)' \
  --run-ignored only --test-threads 1

# Verified sender input must survive real binding policy updates and ledger replay.
CHRONO_TEST_MONGODB_URI="$CMA_IM_TEST_MONGODB_URI" \
  contract_nextest am run --run-ignored only --test-threads 1 --no-fail-fast \
  -E 'package(=cma) and test(~sender_context_tests::)'

# The gateway reset journey also opens M1's store on this authenticated fixture.
CHRONO_TEST_MONGODB_URI="$CMA_IM_TEST_MONGODB_URI" \
  contract_nextest am run --run-ignored only --test-threads 1 --no-fail-fast \
  -E 'package(=cma) and (test(=store::bindings::reset_tests::gateway_reset_tests::configured_gateway_replays_real_m1_reset_and_preserves_both_scope_floors) or test(=store::bindings::reset_tests::gateway_reset_tests::profile_reset_receipt_replays_after_a_later_channel_revision))'

contract_nextest am run --run-ignored only --test-threads 1 --no-fail-fast \
  -E 'package(=cma) and test(=auth::account_standing::mongo_tests::replicas::fresh_snapshot_cannot_mask_real_database_permission_failure)'

# Fixed-pin Desktop startup must work under a role denied the release collection.
CHRONO_TEST_MONGODB_URI="$CMA_IM_TEST_MONGODB_URI" \
  contract_nextest am run --run-ignored only --test-threads 1 --no-fail-fast \
  -E 'package(=cma) and test(=store::native_warm_pools::retirement::machine_selection::tests::fixed_pin_machine_selection_works_without_released_runtime_grant)'

CHRONO_TEST_MONGODB_URI="$CMA_IM_TEST_MONGODB_URI" \
  CMA_TEST_CMA_EXECUTABLE="${CARGO_TARGET_DIR:-$PWD/target}/debug/cma" \
  contract_nextest am run --run-ignored only --test-threads 1 --no-fail-fast \
  -E 'package(=cma) and test(=store::agent_log_segments::active_proof::prepare::tests::active_proof_operator_cli_uses_generated_isolated_role)'
