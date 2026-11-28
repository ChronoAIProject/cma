#!/usr/bin/env bash
set -euo pipefail

script_directory=$(dirname -- "${BASH_SOURCE[0]}")
repo_root=$(CDPATH='' cd -- "${script_directory}/../.." && pwd -P)
readonly repo_root
unset CHRONO_TEST_MONGODB_URI CHRONO_TEST_MONGODB_FAILPOINT_URI CMA_AUTHENTICATED_TEST_MONGODB_URI
# Retries come only from the explicit merge-queue flag below, never the environment.
unset NEXTEST_RETRIES NEXTEST_FLAKY_RESULT
# Owner-approved merge-queue flaky retry: only the workflow's policy step
# (.github/scripts/mongo_flaky_retry.py policy) sets 1, for an ordinary
# merge-group shard before CMA_MONGO_FLAKY_RETRY_UNTIL. Unset or 0 runs none.
case "${CMA_MONGO_FLAKY_RETRIES:-}" in
  ''|0) ;;
  1)
    if [[ ! "${CMA_MONGO_FLAKY_REPORT_DIR:-}" =~ ^/[A-Za-z0-9._/-]+$ || ! -d "$CMA_MONGO_FLAKY_REPORT_DIR" ]]; then
      echo 'CMA_MONGO_FLAKY_RETRIES=1 needs an existing absolute CMA_MONGO_FLAKY_REPORT_DIR' >&2
      exit 2
    fi
    ;;
  *) echo 'CMA_MONGO_FLAKY_RETRIES must be unset, 0 or 1' >&2; exit 2 ;;
esac
# shellcheck source=.github/scripts/backend-nextest.sh
source "$script_directory/backend-nextest.sh"

# Listing and execution share one complete selection. The coordinator sizes
# disposable fixtures from this list, keeping database metadata growth bounded.
readonly scale_test='store::usage_reports::tests::scale::metrics_over_twenty_thousand_sandboxes_generate_full_cohort_and_indexed_pages'
readonly owner_allocation_test='store::usage_reports::tests::owner_allocation::metrics_owner_allocations_full_cohort_at_400_and_20001_is_bounded_and_cursor_bound'
readonly audit_bounds_test='store::agent_log_segments::mongo_tests::audit_bounds::near_capacity_audit_keeps_source_bounds_and_limits_each_write_transaction'
readonly diagnostics_test='store::mongo_diagnostics::mongo_tests::real_mongo_monitoring_privacy_and_overhead'
readonly scan_cooperation_test='store::agent_log_telemetry::tests::cooperation::benchmark::local_scan_scheduling_abba_reproduction'
readonly bson_ceiling_test='store::sandbox_mongo_integration_tests::resource_access_tests::personal_inventory::failures::personal_agent_list_preserves_sources_near_the_bson_document_ceiling'
# On-demand qualifications never enter a PR shard: the listing (all) and the
# ordinary partitions both exclude them, so they run only through their mode.
readonly observation_budget_test='store::sandbox_mongo_integration_tests::resource_access_tests::history_measurement::observation_budget::observation_one_hundred_observers_on_two_replicas_for_thirty_minutes'
readonly first_response_test='store::usage_reports::tests::submission::default_day_report_serves_four_hundred_sandboxes_without_polling'
readonly history_test='store::usage_reports::tests::history_batch::seven_day_history_generates_and_reuses_complete_421_member_report'
readonly ranked_projection_test='store::usage_reports::tests::projection_scale::aggregate_projection_ranges_measure_sparse_and_dense_frozen_sources'
readonly tc070_control_test='store::sandbox_mongo_integration_tests::command_ledger_tests::targeted_control_tests::capacity_control_qualification::tc070_admitted_controls_remain_bounded_at_retained_home_capacity'
readonly response_replay_test='store::sandbox_mongo_integration_tests::resource_access_tests::history_measurement::observation_budget::response_shared::shared_adopted_response_complete_routes_meet_unchanged_budgets_and_exact_replay'
selection='all'
suite_filter=$(python3 "$script_directory/mongo-contract-registry.py" --filter)
readonly suite_filter
# Keep this nextest budget for ordinary partitions and the isolated ranked
# projection and response replay measurements. Other modes use the default.
readonly ordinary_profile='mongo-ordinary'
# Nextest prints no custom text when it terminates a test at the budget.
contract_failure() {
  local status=$1
  if [[ "$selection" == ordinary ]]; then
    printf '%s\n' >&2 \
      "Mongo partition $partition failed (exit $status). A TIMEOUT above is a test that exceeded" \
      "the per-test budget of nextest profile $ordinary_profile in api/.config/nextest.toml." \
      "Do not raise the budget: move the long measurement or qualification into a dedicated mode" \
      "in .github/scripts/mongo-contract-tests.sh (a readonly name, a selection case, the" \
      "ordinary/all exclusions and an argument) and declare it in mongo-contract-exceptions.json."
  fi
  return "$status"
}
nextest_contracts() {
  local selection_filter="not (test(=store::sandbox_mongo_integration_tests::resource_access_tests::personal_inventory::concurrency::personal_agent_list_phase_profile) or test(=store::sandbox_mongo_integration_tests::resource_access_tests::personal_inventory::concurrency::personal_agent_provenance_phase_profile) or test(=store::sandbox_mongo_integration_tests::resource_access_tests::personal_inventory::concurrency::personal_agent_list_one_hundred_concurrent_public_readers_meet_budgets) or test(=store::sandbox_mongo_integration_tests::resource_access_tests::personal_inventory::concurrency::personal_agent_provenance_one_hundred_concurrent_public_readers_meet_budgets) or test(=store::sandbox_mongo_integration_tests::resource_access_tests::personal_inventory::availability::personal_inventory_constrained_availability_comparison) or test(=store::sandbox_mongo_integration_tests::resource_access_tests::personal_inventory::organization::organization_one_hundred_concurrent_public_readers_meet_budgets) or test(=$observation_budget_test))"
  case "$selection" in
    scale) selection_filter="package(=cma) and test(=$scale_test)" ;;
    owner-allocation) selection_filter="package(=cma) and test(=$owner_allocation_test)" ;;
    audit-bounds) selection_filter="package(=cma) and test(=$audit_bounds_test)" ;;
    diagnostics) selection_filter="package(=cma) and test(=$diagnostics_test)" ;;
    scan-cooperation) selection_filter="package(=cma) and test(=$scan_cooperation_test)" ;;
    history) selection_filter="package(=cma) and test(=$history_test)" ;;
    ranked-projection) selection_filter="package(=cma) and test(=$ranked_projection_test)" ;;
    tc070-control) selection_filter="package(=cma) and test(=$tc070_control_test)" ;;
    response-replay) selection_filter="package(=cma) and test(=$response_replay_test)" ;;
    first-response) selection_filter="package(=cma) and test(=$first_response_test)" ;;
    bson-ceiling) selection_filter="package(=cma) and test(=$bson_ceiling_test)" ;;
    agent-list-contracts) selection_filter='package(=cma) and test(::resource_access_tests::personal_inventory::contracts::)' ;;
    availability) selection_filter='package(=cma) and test(::personal_inventory::availability::)' ;;
    agent-list-profile) selection_filter='package(=cma) and (test(::resource_access_tests::personal_inventory::concurrency::personal_agent_list_phase_profile) or test(::resource_access_tests::personal_inventory::concurrency::personal_agent_provenance_phase_profile))' ;;
    organization-inventory) selection_filter='package(=cma) and (test(::resource_access_tests::inventory::) or test(::resource_access_tests::personal_inventory::organization::))' ;;
    agent-lists) selection_filter='package(=cma) and not test(::personal_inventory::availability::) and not (test(::resource_access_tests::personal_inventory::concurrency::personal_agent_list_phase_profile) or test(::resource_access_tests::personal_inventory::concurrency::personal_agent_provenance_phase_profile)) and (test(::resource_access_tests::personal_inventory::) or test(::resource_access_tests::inventory::) or test(::resource_access_tests::provenance::) or test(::agent_task_status::batch::tests::) or test(::agent_task_status::activity::tests::) or test(::agent_admission::mongo_integration_tests::channel_usage::) or test(::agent_admission::mongo_integration_tests::provenance_inventory::))' ;;
    request-limits) selection_filter='package(=cma) and (test(store::request_limits::tests::) or test(rate_limit::lease::mongo_tests::) or test(rate_limit::anonymous_mongo_tests::) or test(rate_limit::control_tests::mongo::) or test(rate_limit::input_tests::mongo::))' ;;
    cpu-profile) selection_filter="package(=cma) and test(::cpu_profile::)" ;;
    observation-budget) selection_filter="package(=cma) and test(=$observation_budget_test)" ;;
    ordinary) selection_filter="not (test(=$scale_test) or test(=$owner_allocation_test) or test(=$audit_bounds_test) or test(=$diagnostics_test) or test(=$scan_cooperation_test) or test(=$bson_ceiling_test) or test(=$first_response_test) or test(=$history_test) or test(=$ranked_projection_test) or test(=$tc070_control_test) or test(=$response_replay_test)) & not (test(=store::sandbox_mongo_integration_tests::resource_access_tests::personal_inventory::concurrency::personal_agent_list_phase_profile) or test(=store::sandbox_mongo_integration_tests::resource_access_tests::personal_inventory::concurrency::personal_agent_provenance_phase_profile) or test(=store::sandbox_mongo_integration_tests::resource_access_tests::personal_inventory::concurrency::personal_agent_list_one_hundred_concurrent_public_readers_meet_budgets) or test(=store::sandbox_mongo_integration_tests::resource_access_tests::personal_inventory::concurrency::personal_agent_provenance_one_hundred_concurrent_public_readers_meet_budgets) or test(=store::sandbox_mongo_integration_tests::resource_access_tests::personal_inventory::availability::personal_inventory_constrained_availability_comparison) or test(=store::sandbox_mongo_integration_tests::resource_access_tests::personal_inventory::organization::organization_one_hundred_concurrent_public_readers_meet_budgets) or test(=$observation_budget_test))" ;;
  esac
  # The two-Manager job repeats these tests without removing them from the ordinary corpus.
  if [[ "$selection" == prepared-startup ]]; then
    selection_filter=$prepared_startup_filter
  fi
  if [[ "$selection" == ordinary || "$selection" == ranked-projection || "$selection" == response-replay ]]; then
    set -- "$@" --profile "$ordinary_profile"
  fi
  # One retry, for ordinary partitions only and never for a dedicated mode.
  # The partition's JUnit report records each pass on retry as flakyFailure,
  # which the workflow flags; the tool config sets only that report's path.
  if [[ "$selection" == ordinary && "$1" == run && "${CMA_MONGO_FLAKY_RETRIES:-}" == 1 ]]; then
    local flaky_stem=${partition//[:\/]/-}
    local flaky_config="$CMA_MONGO_FLAKY_REPORT_DIR/nextest-$flaky_stem.toml"
    printf '[profile.%s.junit]\npath = "%s"\n' "$ordinary_profile" \
      "$CMA_MONGO_FLAKY_REPORT_DIR/junit-$flaky_stem.xml" > "$flaky_config"
    set -- "$@" --retries 1 --flaky-result pass --tool-config-file "cma-flaky-retry:$flaky_config"
  fi
  # Isolated role proof belongs to authenticated-mongo-contract.sh, which owns
  # an authenticated fixture. Never run it against this unauthenticated set.
  # Process children need their parent's request/database; their parents invoke
  # them explicitly. Desktop latency is a separate matched-measurement fixture.
  nextest_build "$@" --run-ignored only \
    -E "$selection_filter & not test(=auth::account_standing::mongo_tests::replicas::fresh_snapshot_cannot_mask_real_database_permission_failure) & not test(=store::native_warm_pools::retirement::machine_selection::tests::fixed_pin_machine_selection_works_without_released_runtime_grant) & not test(=agent_manager::relay::tests::mongo::preferences::process_recovery::child::preferences_replica_child) & not test(=store::bindings::reset_tests::gateway_reset_tests::configured_gateway_replays_real_m1_reset_and_preserves_both_scope_floors) & not test(=store::bindings::reset_tests::gateway_reset_tests::profile_reset_receipt_replays_after_a_later_channel_revision) & not test(=store::sandbox_usage::raw_event::tests::allocation_raw_event_cpu_comparison) & not test(=store::usage_reports::tests::hotfix_cpu::native_usage_hotfix_decode_phase_calibration) & not test(=store::usage_reports::tests::hotfix_matrix::native_usage_hotfix_full_report_six_case_matrix) & not test(store::migrations::tests::segments_runtime::) & not test(=sandbox::agent_worker_tests::sdk_conformance_tests::task_home_tests::subprocess::task_home_create_child) & not test(=store::sandbox_sessions::exclusive::process_tests::desktop_claim_child) & not test(=store::sandbox_sessions::exclusive::service_tests::latency_tests::repeated_desktop_connect_windows) & not test(=store::sandbox_sessions::exclusive::service_tests::endpoint_tests::public_disabled_services_and_ownerless_receiver_refuse_new_streams) & not test(=store::sandbox_sessions::exclusive::service_tests::endpoint_tests::retained_legacy_rows_block_until_close_or_expiry_and_cannot_release_successor) & not test(=store::sandbox_sessions::exclusive::service_tests::endpoint_tests::public_replica_handoff_and_revocation_use_actual_receiver) & not test(=store::sandbox_sessions::exclusive::service_tests::endpoint_tests::public_uncertain_confirm_and_receiver_restart_preserve_fencing) & not test(=store::sandbox_sessions::exclusive::service_tests::endpoint_tests::delayed_receiver_renewal_ack_cannot_restore_public_authority) & not test(=store::sandbox_sessions::exclusive::service_tests::endpoint_tests::measurements::matched_public_desktop_connect_windows) & not test(=store::sandbox_mongo_integration_tests::command_ledger_tests::execution_tests::stale_turn_tests::native_stop_trace::native_stop_trace_recorder_ack_matches_a_fresh_checkpoint) & not test(=store::agent_log_segments::active_proof::prepare::tests::active_proof_operator_cli_uses_generated_isolated_role) & ($suite_filter)" || contract_failure "$?"
}
if [[ $# == 0 || ( $# == 2 && "$1" == '--shard' ) ]]; then
  exec python3 "$script_directory/mongo-contract-partitions.py" "${BASH_SOURCE[0]}" "$@"
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
elif [[ $# == 1 && "$1" == '--audit-bounds' ]]; then
  selection='audit-bounds'
elif [[ $# == 1 && "$1" == '--diagnostics' ]]; then
  selection='diagnostics'
elif [[ $# == 1 && "$1" == '--scan-cooperation' ]]; then
  selection='scan-cooperation'
elif [[ $# == 1 && "$1" == '--ranked-projection' ]]; then
  selection='ranked-projection'
elif [[ $# == 1 && "$1" == '--tc070-control' ]]; then
  selection='tc070-control'
elif [[ $# == 1 && "$1" == '--response-replay' ]]; then
  selection='response-replay'
elif [[ $# == 1 && "$1" == '--history' ]]; then
  selection='history'
elif [[ $# == 1 && "$1" == '--first-response' ]]; then
  selection='first-response'
elif [[ $# == 1 && "$1" == '--bson-ceiling' ]]; then
  selection='bson-ceiling'
elif [[ $# == 1 && "$1" == '--agent-list-contracts' ]]; then
  selection='agent-list-contracts'
elif [[ $# == 1 && "$1" == '--availability' ]]; then
  selection='availability'
elif [[ $# == 1 && "$1" == '--agent-list-profile' ]]; then
  selection='agent-list-profile'
elif [[ $# == 1 && "$1" == '--organization-inventory' ]]; then
  selection='organization-inventory'
elif [[ $# == 1 && "$1" == '--agent-lists' ]]; then
  selection='agent-lists'
elif [[ $# == 1 && "$1" == '--request-limits' ]]; then
  selection='request-limits'
elif [[ $# == 1 && "$1" == '--cpu-profile' ]]; then
  selection='cpu-profile'
elif [[ $# == 1 && "$1" == '--observation-budget' ]]; then
  selection='observation-budget'
elif [[ $# == 1 && "$1" == '--prepared-startup' ]]; then
  selection='prepared-startup'
else
  [[ $# == 1 && "$1" =~ ^count:([1-9][0-9]{0,5})/([1-9][0-9]{0,5})$ ]] || exit 2
  [[ ${BASH_REMATCH[1]} -le ${BASH_REMATCH[2]} ]] || exit 2
  selection='ordinary'
  partition=$1
fi

if [[ "${CMA_NEXTEST_REUSE:-}" != 1 && ( "$selection" == organization-inventory || "$selection" == agent-list-contracts || "$selection" == availability || "$selection" == agent-lists || "$selection" == agent-list-profile || "$selection" == prepared-startup || "$selection" == response-replay ) ]]; then
  nextest_build_args=(--locked -p cma)
fi
if [[ "$selection" == 'prepared-startup' ]]; then
  prepared_startup_filter=$(python3 "$script_directory/multi-am-features/startup_controls.py" --filter)
  nextest_contracts list --message-format json |
    python3 "$script_directory/multi-am-features/startup_controls.py" --check-listing
fi

# This entry point always creates its own disposable replica set. It does not
# accept a caller's Mongo URI, credentials, volume or existing container.
readonly mongo_image='mongo:8.0.13@sha256:cf340b1e5283843c63eb12999922f20c463ae31285f746d30f05dcc21cd1d47c'
contract_suffix=$(openssl rand -hex 12)
[[ "$contract_suffix" =~ ^[0-9a-f]{24}$ ]] || exit 1
readonly contract_name="cma-mongo-ci-${contract_suffix}"
readonly contract_label="dev.chrono.cma.mongo-contract=${contract_name}"
readonly contract_volume="${contract_name}-data"
readonly mongo_cpus="${CMA_MONGO_CPUS:-2}"
[[ "$mongo_cpus" =~ ^[1-9][0-9]*([.][0-9]+)?$ ]] || exit 2
# Keep the cache choice with the fixture body used by the predecessor runner.
# The near-16 MiB replacement can exhaust the ordinary dirty-cache allowance.
if [[ "$selection" == 'bson-ceiling' ]]; then
  readonly wired_tiger_cache='0.50'
else
  readonly wired_tiger_cache='0.25'
fi

# TC-070 opens and drops hundreds of indexed databases. Give this isolated
# fixture enough memory to finish while ordinary fixtures keep their 2 GiB cap.
if [[ "$selection" == 'tc070-control' ]]; then
  readonly mongo_memory='4g'
else
  readonly mongo_memory='2g'
fi

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
bash "$repo_root/.github/scripts/pull-pinned-image.sh" "$mongo_image"
docker run --detach --name "$contract_name" --label "$contract_label" \
  --publish 127.0.0.1::27017 --memory "$mongo_memory" --memory-swap "$mongo_memory" --cpus "$mongo_cpus" --pids-limit 256 \
  --ulimit nofile=64000:64000 \
  --mount "type=volume,source=${contract_volume},target=/data/db" \
  --tmpfs /data/configdb:rw,size=16777216 \
  "$mongo_image" --replSet cma_ci --bind_ip_all \
  --wiredTigerCacheSizeGB "$wired_tiger_cache" --oplogSize 64 \
  --setParameter enableTestCommands=1 >/dev/null
readiness_deadline=$((SECONDS + 60))
until docker exec "$contract_name" mongosh --quiet --eval 'db.adminCommand({ping:1}).ok' >/dev/null 2>&1; do
  (( SECONDS < readiness_deadline )) || exit 1
  sleep 0.25
done
docker exec "$contract_name" mongosh --quiet --eval \
  'rs.initiate({_id:"cma_ci",members:[{_id:0,host:"localhost:27017"}]})' >/dev/null
readiness_deadline=$((SECONDS + 60))
until mongo_server=$(docker exec "$contract_name" mongosh --quiet --eval \
  'const hello = db.hello(); if (!hello.isWritablePrimary) quit(1); print(hello.topologyVersion.processId.toHexString(), hello.localTime.getTime())' 2>/dev/null); do
  (( SECONDS < readiness_deadline )) || exit 1
  sleep 0.25
done
[[ "$mongo_server" =~ ^([0-9a-f]{24})\ ([0-9]{10,16})$ ]] || exit 1
mongo_process=${BASH_REMATCH[1]}
mongo_unix_ms=${BASH_REMATCH[2]}
endpoint=$(docker port "$contract_name" 27017/tcp)
[[ "$endpoint" =~ ^127\.0\.0\.1:[0-9]+$ ]] || exit 1
export CHRONO_TEST_MONGODB_URI="mongodb://${endpoint}/?directConnection=true&replicaSet=cma_ci"
export CHRONO_TEST_MONGODB_FAILPOINT_URI="$CHRONO_TEST_MONGODB_URI"
partition_name=${partition:-$selection}
shard=${CMA_MONGO_COORDINATOR_SHARD:-standalone}
[[ "$shard" =~ ^([1-8]/8|all|isolated|standalone)$ ]] || shard=standalone
python3 -B - "$repo_root" "$contract_suffix" "$mongo_process" "$mongo_unix_ms" "$partition_name" "$shard" "$mongo_cpus" <<'PY'
from hashlib import sha256
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time

root, fixture, process, mongo_time, partition, shard, cpus = sys.argv[1:]
host_time = time.time_ns() // 1_000_000
revision = clean = metadata = host_load = None
try:
    commit = subprocess.run(["git", "rev-parse", "--verify", "HEAD"], cwd=root,
                            capture_output=True, text=True, check=True, timeout=5).stdout.strip()
    if re.fullmatch(r"[0-9a-f]{40}", commit):
        revision = commit
        tracked = subprocess.run(["git", "status", "--porcelain", "--untracked-files=no"], cwd=root,
                                 capture_output=True, check=True, timeout=5)
        clean = not tracked.stdout
except (OSError, subprocess.SubprocessError):
    pass
try:
    if os.environ.get("CMA_NEXTEST_REUSE") == "1":
        metadata = sha256((Path(root) / "api/target/nextest/binaries-metadata.json").read_bytes()).hexdigest()
    host_load = [round(value, 2) for value in os.getloadavg()]
except OSError:
    pass
print("Mongo fixture " + json.dumps({
    "event": "started", "fixture": fixture, "partition": partition, "shard": shard,
    "checkout_sha": revision, "tracked_checkout_clean": clean, "archive_metadata_sha256": metadata,
    "mongo_process": process, "mongo_unix_ms": int(mongo_time), "mongo_cpus": cpus,
    "host_load": host_load, "host_unix_ms": host_time,
}), flush=True)
PY
test_started=$SECONDS
test_status=0
cd "$repo_root/api"
if [[ "$selection" == 'availability' ]]; then
  nextest_contracts run --test-threads 1 --no-fail-fast --no-capture || test_status=$?
elif [[ "$selection" == 'ordinary' ]]; then
  nextest_contracts run --test-threads 1 --no-fail-fast --partition "$partition" || test_status=$?
else
  nextest_contracts run --test-threads 1 --no-fail-fast --success-output final || test_status=$?
fi
printf 'Mongo fixture {"event":"finished","fixture":"%s","partition":"%s","status":%d,"elapsed_seconds":%d}\n' \
  "$contract_suffix" "$partition_name" "$test_status" "$((SECONDS - test_started))"
exit "$test_status"
