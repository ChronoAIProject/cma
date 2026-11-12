#!/usr/bin/env bash
set -euo pipefail

script_directory=$(dirname -- "${BASH_SOURCE[0]}")
repo_root=$(CDPATH='' cd -- "${script_directory}/../.." && pwd -P)
readonly repo_root
unset CHRONO_TEST_MONGODB_URI CHRONO_TEST_MONGODB_FAILPOINT_URI CMA_IM_TEST_MONGODB_URI
# shellcheck source=.github/scripts/backend-nextest.sh
source "$script_directory/backend-nextest.sh"

# Listing and execution share one complete selection. The coordinator sizes
# disposable fixtures from this list, keeping database metadata growth bounded.
readonly scale_test='store::usage_reports::tests::scale::metrics_over_twenty_thousand_sandboxes_generate_full_cohort_and_indexed_pages'
readonly owner_allocation_test='store::usage_reports::tests::owner_allocation::metrics_owner_allocations_full_cohort_at_400_and_20001_is_bounded_and_cursor_bound'
readonly diagnostics_test='store::mongo_diagnostics::mongo_tests::real_mongo_monitoring_privacy_and_overhead'
selection='all'
nextest_contracts() {
  local selection_filter='all()'
  case "$selection" in
    scale) selection_filter="package(=cma) and test(=$scale_test)" ;;
    owner-allocation) selection_filter="package(=cma) and test(=$owner_allocation_test)" ;;
    diagnostics) selection_filter="package(=cma) and test(=$diagnostics_test)" ;;
    cpu-profile) selection_filter="package(=cma) and test(::cpu_profile::)" ;;
    ordinary) selection_filter="not (test(=$scale_test) or test(=$owner_allocation_test) or test(=$diagnostics_test))" ;;
  esac
  # Isolated role proof belongs to image-manager-mongo-contract.sh, which owns
  # an authenticated fixture. Never run it against this unauthenticated set.
  nextest_build "$@" --run-ignored only \
    -E "$selection_filter & not test(store::migrations::tests::segments_runtime::) & ("'test(store::usage_reports::tests::) | test(sandbox::funding::tests::mongo::) | test(usage_billing::tests::) | test(store::request_limits::tests::) | test(store::sandbox_mongo_integration_tests::sandbox_usage_tests::) | test(document_artifacts::mongo_integration_tests::) | test(agent_access::tests::tenant_isolation::) | test(store::message_feedback::tests::) | test(store::sandbox_colors::tests::) | test(agent_lifecycle::workspace::tests::mongo::) | test(agent_lifecycle::workspace::tests::bootstrap::) | test(warm_pools::tests::mongo::) | test(store::native_warm_pools::retirement::tests::) | test(store::native_prepared::tests::) | test(store::sandbox_admission_mongo_integration_tests::warmup::) | test(store::auth::api_bearer_tests::) | test(store::agent_conversation_store::checkpoint_tests::) | test(store::agent_conversation_store::admission_tests::) | test(store::agent_conversation_store::reopen_tests::) | test(store::agent_access_tools::) | test(store::agent_access_requests::) | test(agent_access_worker::) | test(owner_service_access::) | test(store::sandbox_mongo_integration_tests::warm_hierarchy_tests::) | test(store::agent_admission::mongo_integration_tests::) | test(store::schema_mongo_integration_tests::) | test(store::migrations::tests::) | test(store::retirement_archive::tests::) | test(store::sandbox_mongo_integration_tests::ledger_retirement_tests::) | test(store::sandbox_mongo_integration_tests::ledger_census_tests::) | test(store::workspaces::mongo_integration_tests::) | test(store::bindings::) | test(store::gateway_records_mongo_integration_tests::) | test(store::bots_mongo_integration_tests::) | test(store::bot_setups::mongo_integration_tests::) | test(bot_setups::events::tests::) | test(store::trigger_definitions::launch_mongo_integration_tests::) | test(store::trigger_definitions::archive_mongo_tests::) | test(store::checkpoint_objects::mongo_integration_tests::) | test(store::sandbox_mongo_integration_tests::checkpoint_capture_tests::) | test(store::sandbox_mongo_integration_tests::service_access_tests::) | test(store::sandbox_mongo_integration_tests::command_ledger_tests::) | test(store::sandbox_mongo_integration_tests::resource_access_tests::) | test(store::sandbox_maintenance::mongo_integration_tests::) | test(store::sandbox_maintenance::consented::scope_tests::) | test(store::sandbox_reconciliation::mongo_integration_tests::) | test(sandbox::agent_worker_tests::sdk_conformance_tests::mongo_tests::) | test(sandbox::agent_worker_tests::first_message_contention_tests::) | test(sandbox::worker_tests::lifecycle_queue_contention_tests::) | test(store::sandbox_mongo_integration_tests::sandbox_lifecycle_tests::) | test(sandbox::agent_worker::prepared_open_ready::) | test(store::mongo_latency::tests::) | test(store::mongo_diagnostics::mongo_tests::) | test(store::worker_observations::tests::) | test(store::agent_log_telemetry::tests::) | test(store::agent_log_integrity::quarantine_tests::) | test(store::agent_log::receipt_tests::) | test(store::mongo_read_telemetry::cursor_tests::) | test(store::sandbox_mongo_integration_tests::read_telemetry_tests::) | test(store::agent_task_status::batch::tests::) | test(store::sandbox_mongo_integration_tests::agent_worker_tests::) | test(store::sandbox_mongo_integration_tests::agent_transaction_tests::) | test(store::sandbox_mongo_integration_tests::agent_log_read_tests::) | test(store::sandbox_mongo_integration_tests::response_projection_tests::) | test(store::agent_log_segment_reads::transcript::tests::) | test(store::suggestions::context::tests::) | test(store::agent_log_segments::mongo_tests::) | test(store::agent_log_segments::command_evidence::points_tests::) | test(store::agent_log_segments::writer_commit::persistence::tests::) | test(store::sandbox_mongo_integration_tests::agent_capacity_recovery_tests::) | test(store::sandbox_mongo_integration_tests::agent_segment_tests::) | test(store::sandbox_mongo_integration_tests::log_integrity_tests::) | test(store::sandbox_mongo_integration_tests::agent_title_tests::) | test(store::retained_reopen::) | test(store::sandbox_unpublished_recovery::mongo_integration_tests::retained::) | test(sandbox::agent_worker_tests::connection_failure_tests::capacity_cleanup_tests::)'")"
}
if [[ $# == 0 ]]; then
  exec python3 "$script_directory/mongo-contract-partitions.py" "${BASH_SOURCE[0]}"
fi
if [[ $# == 1 && "$1" == '--list' ]]; then
  cd "$repo_root/api"
  nextest_contracts list --message-format json
  exit
fi
if [[ $# == 1 && "$1" == '--scale' ]]; then
  selection='scale'
elif [[ $# == 1 && "$1" == '--owner-allocation' ]]; then
  selection='owner-allocation'
elif [[ $# == 1 && "$1" == '--diagnostics' ]]; then
  selection='diagnostics'
elif [[ $# == 1 && "$1" == '--cpu-profile' ]]; then
  selection='cpu-profile'
else
  [[ $# == 1 && "$1" =~ ^count:([1-9][0-9]{0,5})/([1-9][0-9]{0,5})$ ]] || exit 2
  [[ ${BASH_REMATCH[1]} -le ${BASH_REMATCH[2]} ]] || exit 2
  selection='ordinary'
  partition=$1
fi

# This entry point always creates its own disposable replica set. It does not
# accept a caller's Mongo URI, credentials, volume or existing container.
readonly mongo_image='mongo:8.0.13@sha256:cf340b1e5283843c63eb12999922f20c463ae31285f746d30f05dcc21cd1d47c'
contract_suffix=$(openssl rand -hex 12)
[[ "$contract_suffix" =~ ^[0-9a-f]{24}$ ]] || exit 1
readonly contract_name="cma-mongo-ci-${contract_suffix}"
readonly contract_label="dev.chrono.cma.mongo-contract=${contract_name}"
readonly contract_volume="${contract_name}-data"

# Refuse an existing name before establishing a cleanup scope or creating data.
existing_volume=$(docker volume ls --quiet --filter "name=^${contract_volume}$")
[[ -z "$existing_volume" ]] || exit 1

cleanup() {
  local status=$? ids remaining id volume owner
  # 125 tells the coordinator cleanup is uncertain, so no new phase may start.
  trap - EXIT
  trap '' INT TERM
  if ! ids=$(docker ps --all --quiet --filter "label=${contract_label}"); then
    exit 125
  fi
  for id in $ids; do
    [[ "$id" =~ ^[0-9a-f]{12,64}$ ]] || exit 125
    docker rm --force "$id" >/dev/null || status=125
  done
  remaining=$(docker ps --all --quiet --filter "label=${contract_label}") || exit 125
  [[ -z "$remaining" ]] || status=125
  volume=$(docker volume ls --quiet --filter "name=^${contract_volume}$") || exit 125
  if [[ -n "$volume" ]]; then
    [[ "$volume" == "$contract_volume" ]] || exit 125
    owner=$(docker volume inspect --format '{{index .Labels "dev.chrono.cma.mongo-contract"}}' "$volume") || exit 125
    [[ "$owner" == "$contract_name" ]] || exit 125
    docker volume rm "$volume" >/dev/null || status=125
  fi
  remaining=$(docker volume ls --quiet --filter "name=^${contract_volume}$") || exit 125
  [[ -z "$remaining" ]] || status=125
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# Each partition keeps its growing database on owned disk, not VM memory.
# Mongo requires at least 0.25 GiB of WiredTiger cache; bound total memory too.
docker volume create --label "$contract_label" "$contract_volume" >/dev/null
volume_owner=$(docker volume inspect --format '{{index .Labels "dev.chrono.cma.mongo-contract"}}' "$contract_volume")
[[ "$volume_owner" == "$contract_name" ]] || exit 1
docker run --detach --name "$contract_name" --label "$contract_label" \
  --publish 127.0.0.1::27017 --memory 2g --memory-swap 2g --cpus 2 --pids-limit 256 \
  --ulimit nofile=64000:64000 \
  --mount "type=volume,source=${contract_volume},target=/data/db" \
  --tmpfs /data/configdb:rw,size=16777216 \
  "$mongo_image" --replSet cma_ci --bind_ip_all \
  --wiredTigerCacheSizeGB 0.25 --oplogSize 64 \
  --setParameter enableTestCommands=1 >/dev/null
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
if [[ "$selection" == 'scale' || "$selection" == 'owner-allocation' || "$selection" == 'diagnostics' || "$selection" == 'cpu-profile' ]]; then
  nextest_contracts run --test-threads 1 --no-fail-fast --success-output final
else
  nextest_contracts run --test-threads 1 --no-fail-fast --partition "$partition"
fi
