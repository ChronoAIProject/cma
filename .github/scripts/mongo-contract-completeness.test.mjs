import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { requiredPrefixes } from './mongo-contract-test-prefixes.mjs';

const registeredFilter = execFileSync('python3', [
  fileURLToPath(new URL('mongo-contract-registry.py', import.meta.url)), '--filter',
], {encoding: 'utf8'}).trim();

const taskHome = 'sandbox::agent_worker_tests::sdk_conformance_tests::task_home_tests::';

test('capacity recovery completion contracts remain in the ordinary selector', () => {
  const root = 'store::sandbox_mongo_integration_tests::agent_capacity_recovery_tests::composition::quota::';
  const runner = readFileSync(new URL('mongo-contract-tests.sh', import.meta.url), 'utf8');
  for (const suffix of [
    'full_workspace_refuses_recovery_completion_without_acknowledging',
    'races::admission_invalidates_old_completion_snapshot_and_retry_recounts',
    'races::two_recoveries_cannot_publish_into_the_last_slot',
    'races::aborted_completion_does_not_reserve_capacity_for_a_fresh_attempt',
    'retry::capacity_refusal_is_atomic_discoverable_and_completes_after_release',
    'retry::remaining_slot_allows_completion_and_blocks_the_next_ordinary_admission',
    'retry::retained_reservation_refuses_completion_even_with_no_live_agents',
    'retry::available_capacity_cannot_override_parent_removal_or_retirement',
  ]) {
    const name = root + suffix;
    const prefix = requiredPrefixes.find(prefix => name.startsWith(prefix));
    assert.ok(prefix, `${name} must be selected`);
    assert.ok(registeredFilter.includes(`test(${prefix})`));
    assert.ok(!runner.includes(`not test(=${name})`));
    const metadata = inventory();
    metadata['rust-suites']['cma::bin/cma'].testcases[name] = caseMetadata(false);
    const result = check(metadata);
    assert.equal(result.status, 1);
    assert.ok(result.stderr.includes(name), result.stderr);
  }
});

test('Usage continuation Mongo contract remains in the ordinary selector', () => {
  const name = 'store::sandbox_mongo_integration_tests::sandbox_usage_tests::continuation::usage_signed_continuations_cross_replicas_preserve_raw_store_and_reject_scope_changes';
  const prefix = requiredPrefixes.find(prefix => name.startsWith(prefix));
  assert.ok(prefix, 'new Usage Mongo contract must be selected');
  const runner = readFileSync(new URL('mongo-contract-tests.sh', import.meta.url), 'utf8');
  assert.ok(registeredFilter.includes(`test(${prefix})`));
  assert.ok(!runner.includes(`not test(=${name})`));
});
const child = `${taskHome}subprocess::task_home_create_child`;
const caseMetadata = (selected, ignored = true) => ({
  ignored, 'filter-match': {status: selected ? 'matches' : 'mismatch'},
});
const exceptions = {[child]: {kind: 'helper', reason: 'Invoked by its process-exit parent.'}};
function inventory() {
  return {'rust-suites': {'cma::bin/cma': {
    'package-name': 'cma', 'binary-name': 'cma', kind: 'bin',
    testcases: {
      [`${taskHome}subprocess::initial_task_public_service_process_exit_preserves_admission`]: caseMetadata(true),
      'a_new_contract_module::new_mongo_contract': caseMetadata(true),
      [child]: caseMetadata(false),
      'ordinary_unit_test': caseMetadata(false, false),
    },
  }}};
}
function check(metadata, exclusions = exceptions) {
  return spawnSync('python3', ['-c', `
import json, runpy, sys
check = runpy.run_path(sys.argv[1])["check_selection"]
metadata, exceptions = json.load(sys.stdin)
try:
    check(metadata, exceptions)
except (ValueError, KeyError, TypeError) as error:
    raise SystemExit(str(error))
`, fileURLToPath(new URL('mongo-contract-completeness.py', import.meta.url))], {
    input: JSON.stringify([metadata, exclusions]), encoding: 'utf8',
    env: {...process.env, PYTHONDONTWRITEBYTECODE: '1'},
  });
}

test('metadata completeness accepts selected contracts and reports exact helper exclusions', () => {
  const result = check(inventory());
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /3 ignored CMA tests; 2 selected; 0 separate-fixture tests; 1 child helpers; 0 tracked deferrals; 0 unclassified omissions/);
});

test('metadata completeness catches omitted task-home and newly introduced modules independently of the prefix inventory', () => {
  for (const name of [
    `${taskHome}subprocess::initial_task_public_service_process_exit_preserves_admission`,
    'a_new_contract_module::new_mongo_contract',
    'store::trigger_definitions::launch_mongo_integration_tests::pending_home_reconciliation::policy::disabled_startup_with_active_capacity_preserves_usage_receipts_and_ordinary_writers',
    'agent_lifecycle::workspace::tests::quota::quota_preflight_preserves_replay_and_existing_targets_without_preparation',
    'agent_lifecycle::workspace::tests::quota::worker_quota_failure_metadata_is_gated_and_survives_restart',
    'agent_lifecycle::workspace::tests::title_tests::contextual_title_modes_preserve_exact_admission_and_persist_title_authority',
    'agent_lifecycle::workspace::tests::title_tests::admission::explicit_title_modes_require_admission_policy_with_or_without_initial_message',
    'agent_lifecycle::workspace::tests::title_tests::admission::admitted_title_modes_replay_exactly_after_policy_is_withdrawn',
    'agent_lifecycle::workspace::tests::bootstrap::title_tests::bootstrap_title_mode_survives_admission_and_recovery',
    'agent_lifecycle::workspace::tests::bootstrap::title_tests::explicit_new_task_rename_fences_a_late_generated_title',
    `${child}_another_contract`,
  ]) {
    const metadata = inventory();
    metadata['rust-suites']['cma::bin/cma'].testcases[name] = caseMetadata(false);
    const result = check(metadata);
    assert.equal(result.status, 1);
    assert.ok(result.stderr.includes(`Omitted eligible Mongo contracts:\n  ${name}`), result.stderr);
  }
});

test('visitor admission contracts remain covered by the Launch prefix and cannot be omitted', () => {
  const prefix = 'store::trigger_definitions::launch_mongo_integration_tests::';
  assert.ok(requiredPrefixes.includes(prefix));
  for (const name of [
    'faults::transient_failures_keep_the_eight_attempt_baseline_and_enable_sixteen_attempts',
    'faults::transient_insert_retries_but_permanent_driver_and_domain_refusals_do_not',
    'faults::unknown_commit_reconciles_without_replaying_the_admission_body',
    'rollout::disabled_and_enabled_replicas_preserve_quota_replay_and_cancellation_across_days',
    'rollout::disabled_contention_retains_baseline_refusals_without_overspending',
  ]) {
    const fullName = `${prefix}visitor_contention::${name}`;
    const metadata = inventory();
    const cases = metadata['rust-suites']['cma::bin/cma'].testcases;
    cases[fullName] = caseMetadata(true);
    assert.equal(check(metadata).status, 0, fullName);
    cases[fullName] = caseMetadata(false);
    const result = check(metadata);
    assert.equal(result.status, 1, fullName);
    assert.ok(result.stderr.includes(fullName), result.stderr);
  }
});

test('metadata completeness rejects helper selection and stale or undocumented exceptions', () => {
  const metadata = inventory();
  metadata['rust-suites']['cma::bin/cma'].testcases[child] = caseMetadata(true);
  assert.match(check(metadata).stderr, /Selected non-ordinary contracts/);
  assert.match(check(inventory(), {...exceptions, removed: exceptions[child]}).stderr, /Stale exceptions/);
  for (const exception of [{kind: 'helper'}, {kind: 'unknown', reason: 'unknown'}, {kind: 'deferred', reason: 'broken'}]) {
    assert.equal(check(inventory(), {[child]: exception}).status, 1);
  }
});

test('metadata completeness refuses missing binary inventory, malformed test metadata and nonignored selections', () => {
  for (const mutate of [
    metadata => { metadata['rust-suites'] = {}; },
    metadata => { metadata['rust-suites']['cma::bin/cma'].kind = 'lib'; },
    metadata => { metadata['rust-suites']['cma::bin/cma'].testcases = {}; },
    metadata => { metadata['rust-suites']['cma::bin/cma'].testcases[child].ignored = 'true'; },
    metadata => { metadata['rust-suites']['cma::bin/cma'].testcases[child]['filter-match'].status = 'unknown'; },
    metadata => { metadata['rust-suites']['cma::bin/cma'].testcases.ordinary_unit_test = caseMetadata(true, false); },
  ]) {
    const metadata = inventory();
    mutate(metadata);
    assert.equal(check(metadata).status, 1);
  }
});

test('tracked deferrals stay visible instead of being reported as eligible coverage', () => {
  const result = check(inventory(), {[child]: {
    kind: 'deferred', reason: 'Separate tracked failure.',
    issue: 'https://github.com/ChronoAIProject/cma/issues/1854',
  }});
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /1 tracked deferrals/);
  assert.match(result.stdout, /Deferred Mongo contract: .*issues\/1854/);
});

test('every registered suite reaches the runner without a shared suite-count assertion', () => {
  const source = readFileSync(new URL('mongo-contract-tests.sh', import.meta.url), 'utf8');
  const line = source.split('\n').find(line => line.trimStart().startsWith('-E '));
  assert.ok(line.includes('($suite_filter)'));
  const actual = [...registeredFilter.matchAll(/test\(([^)]+)\)/g)].map(match => match[1]);
  assert.deepEqual(actual, requiredPrefixes);
  assert.ok(requiredPrefixes.length > 0);
  assert.equal(new Set(requiredPrefixes).size, requiredPrefixes.length);
  for (const suite of ['store::migrations::command_cohort_index::tests::',
    'store::task_audit::tests::']) {
    assert.ok(requiredPrefixes.includes(suite), suite);
  }
  assert.ok(line.includes(`not test(=${child})`));
});

const desktopChild = 'store::sandbox_sessions::exclusive::process_tests::desktop_claim_child';
const desktopParent = 'store::sandbox_sessions::exclusive::process_tests::independent_processes_preserve_claim_after_holder_exit';
const desktopPerformance = 'store::sandbox_sessions::exclusive::service_tests::latency_tests::repeated_desktop_connect_windows';
const desktopEndpoints = [
  'store::sandbox_sessions::exclusive::service_tests::endpoint_tests::public_disabled_services_and_ownerless_receiver_refuse_new_streams',
  'store::sandbox_sessions::exclusive::service_tests::endpoint_tests::retained_legacy_rows_block_until_close_or_expiry_and_cannot_release_successor',
  'store::sandbox_sessions::exclusive::service_tests::endpoint_tests::public_replica_handoff_and_revocation_use_actual_receiver',
  'store::sandbox_sessions::exclusive::service_tests::endpoint_tests::public_uncertain_confirm_and_receiver_restart_preserve_fencing',
  'store::sandbox_sessions::exclusive::service_tests::endpoint_tests::delayed_receiver_renewal_ack_cannot_restore_public_authority',
  'store::sandbox_sessions::exclusive::service_tests::endpoint_tests::measurements::matched_public_desktop_connect_windows',
];
function desktopInventory() {
  const metadata = inventory();
  Object.assign(metadata['rust-suites']['cma::bin/cma'].testcases, {
    [desktopParent]: caseMetadata(true),
    [desktopChild]: caseMetadata(false),
    [desktopPerformance]: caseMetadata(false),
  });
  const declared = JSON.parse(readFileSync(new URL('mongo-contract-exceptions.json', import.meta.url), 'utf8'));
  for (const name of desktopEndpoints) metadata['rust-suites']['cma::bin/cma'].testcases[name] = caseMetadata(false);
  return [metadata, {...exceptions, [desktopChild]: declared[desktopChild], [desktopPerformance]: declared[desktopPerformance],
    ...Object.fromEntries(desktopEndpoints.map(name => [name, declared[name]]))}];
}

test('desktop selection keeps the process parent while declaring its exact child and matched performance fixture', () => {
  const [metadata, exclusions] = desktopInventory();
  const result = check(metadata, exclusions);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /12 ignored CMA tests; 3 selected; 7 separate-fixture tests; 2 child helpers/);
  const source = readFileSync(new URL('mongo-contract-tests.sh', import.meta.url), 'utf8');
  assert.ok(source.includes(`not test(=${desktopChild})`));
  assert.ok(source.includes(`not test(=${desktopPerformance})`));
  assert.ok(!source.includes(`not test(=${desktopParent})`));
  const runner = readFileSync(new URL('desktop-replica-contract.py', import.meta.url), 'utf8');
  for (const name of desktopEndpoints) {
    assert.ok(source.includes(`not test(=${name})`));
    assert.ok(runner.includes(name.split('::').at(-1)));
    assert.equal(exclusions[name].kind, 'fixture');
  }
});

test('desktop completeness refuses omitted parents, selected children and unclassified sibling contracts', () => {
  for (const [name, selected] of [[desktopParent, false], [desktopChild, true], [desktopPerformance, true],
    ...desktopEndpoints.map(name => [name, true]), [desktopEndpoints[0] + '_new_contract', false], [`${desktopChild}_new_contract`, false]]) {
    const [metadata, exclusions] = desktopInventory();
    metadata['rust-suites']['cma::bin/cma'].testcases[name] = caseMetadata(selected);
    const result = check(metadata, exclusions);
    assert.equal(result.status, 1, name);
    assert.ok(result.stderr.includes(name), result.stderr);
  }
});


test('usage hotfix keeps reader and startup contracts while assigning only the exact large matrix to its fixture', () => {
  const matrix = 'store::usage_reports::tests::hotfix_matrix::native_usage_hotfix_full_report_six_case_matrix';
  const declared = JSON.parse(readFileSync(new URL('mongo-contract-exceptions.json', import.meta.url), 'utf8'));
  const source = readFileSync(new URL('mongo-contract-tests.sh', import.meta.url), 'utf8');
  const names = [
    'store::usage_reports::tests::allocation_interval::allocation_interval_native_cursor_matches_projection_with_and_without_index',
    'store::usage_reports::tests::allocation_interval::allocation_interval_native_boundaries_delta_and_snapshot_remain_exact',
    'store::usage_reports::tests::allocation_cold_range::allocation_cold_range_native_matches_daily_and_declines_any_cache',
    'store::usage_reports::tests::allocation_cold_range::allocation_cold_range_native_snapshot_and_late_corruption_are_not_declines',
    'store::usage_reports::tests::allocation_cold_range::allocation_cold_range_native_combined_cap_declines_without_cache_prefix',
    'store::sandbox_usage::interval_index::tests::interval_index_real_startup_creates_and_reopens_without_replacing_existing_indexes',
    'store::sandbox_usage::interval_index::tests::interval_index_resumes_partial_startup_and_coalesces_concurrent_ensure',
    'store::sandbox_usage::interval_index::tests::interval_index_real_startup_accepts_equivalent_existing_name',
    'store::sandbox_usage::interval_index::tests::interval_index_real_startup_refuses_conflicting_options_without_repair',
  ];
  assert.ok(source.includes(`not test(=${matrix})`));
  assert.equal(declared[matrix].kind, 'fixture');
  const metadata = inventory();
  const cases = metadata['rust-suites']['cma::bin/cma'].testcases;
  cases[matrix] = caseMetadata(false);
  for (const name of names) {
    assert.ok(!source.includes(`not test(=${name})`));
    assert.equal(declared[name], undefined);
    cases[name] = caseMetadata(true);
  }
  const exclusions = {...exceptions, [matrix]: declared[matrix]};
  const cpuFixtures = [
    'store::sandbox_usage::raw_event::tests::allocation_raw_event_cpu_comparison',
    'store::usage_reports::tests::hotfix_cpu::native_usage_hotfix_decode_phase_calibration',
  ];
  for (const name of cpuFixtures) {
    assert.ok(source.includes(`not test(=${name})`));
    assert.equal(declared[name].kind, 'fixture');
    exclusions[name] = declared[name];
    cases[name] = caseMetadata(false);
  }
  assert.equal(check(metadata, exclusions).status, 0);
  for (const name of names) {
    cases[name] = caseMetadata(false);
    assert.equal(check(metadata, exclusions).status, 1, name);
    cases[name] = caseMetadata(true);
  }
  cases[`${matrix}_another_contract`] = caseMetadata(false);
  assert.equal(check(metadata, exclusions).status, 1);
});

const resourceExecutionPrefix = 'store::resource_access::execution::mongo_tests::';
test('standing permission loss uses its authenticated fixture while replica acceptance stays ordinary', () => {
  const prefix = 'auth::account_standing::mongo_tests::replicas::';
  const permission = `${prefix}fresh_snapshot_cannot_mask_real_database_permission_failure`;
  const acceptance = `${prefix}other_replica_acceptance_revokes_fresh_request_stream_frame_and_command`;
  const declared = JSON.parse(readFileSync(new URL('mongo-contract-exceptions.json', import.meta.url), 'utf8'));
  const source = readFileSync(new URL('mongo-contract-tests.sh', import.meta.url), 'utf8');
  assert.equal(declared[permission]?.kind, 'fixture');
  assert.ok(source.includes(`not test(=${permission})`));
  assert.ok(!source.includes(`not test(=${acceptance})`));
  assert.equal(declared[acceptance], undefined);
  const metadata = inventory();
  const cases = metadata['rust-suites']['cma::bin/cma'].testcases;
  cases[permission] = caseMetadata(false);
  cases[acceptance] = caseMetadata(true);
  const exclusions = {...exceptions, [permission]: declared[permission]};
  const result = check(metadata, exclusions);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /5 ignored CMA tests; 3 selected; 1 separate-fixture tests; 1 child helpers/);
  cases[permission] = caseMetadata(true);
  assert.match(check(metadata, exclusions).stderr, /Selected non-ordinary contracts/);
  cases[permission] = caseMetadata(false);
  cases[acceptance] = caseMetadata(false);
  assert.match(check(metadata, exclusions).stderr, /Omitted eligible Mongo contracts/);
});

test('resource execution selection includes root and nested contracts without omissions', () => {
  assert.equal(requiredPrefixes.filter(prefix => prefix === resourceExecutionPrefix).length, 1);
  const source = readFileSync(new URL('mongo-contract-tests.sh', import.meta.url), 'utf8');
  const line = source.split('\n').find(value => value.trimStart().startsWith('-E '));
  const included = [...registeredFilter.matchAll(/test\(([^)]+)\)/g)].map(match => match[1]);
  assert.equal(included.filter(prefix => prefix === resourceExecutionPrefix).length, 1);
  assert.ok(!line.includes(`not test(${resourceExecutionPrefix})`));
  const names = [
    'ticket_continuation_and_release_write_conflict_with_freeze',
    'races::registration_before_freeze_is_retained_until_every_exact_ack',
    'uncertainty::ambiguous_commits_preserve_frozen_members_and_replay_the_exact_final_receipt',
  ].map(name => resourceExecutionPrefix + name);
  const metadata = inventory();
  const cases = metadata['rust-suites']['cma::bin/cma'].testcases;
  for (const name of names) cases[name] = caseMetadata(true);
  const selected = check(metadata);
  assert.equal(selected.status, 0, selected.stderr);
  const missingSuite = structuredClone(metadata);
  delete missingSuite['rust-suites']['cma::bin/cma'];
  assert.equal(check(missingSuite).status, 1);
  for (const omitted of [names, ...names.map(name => [name])]) {
    for (const name of omitted) cases[name] = caseMetadata(false);
    const result = check(metadata);
    assert.equal(result.status, 1);
    for (const name of omitted) assert.ok(result.stderr.includes(name), result.stderr);
    for (const name of omitted) cases[name] = caseMetadata(true);
  }
});

test('journal permission denial is authenticated and ordinary journal contracts remain selected', () => {
  const permission = "store::account_deletions::tests::worker_authority::legacy_role_keeps_non_enrolled_writes_and_journal_role_cannot_index_or_delete";
  const ordinary = 'store::account_deletions::tests::worker_journal::ordinary_journal_contract';
  const declared = JSON.parse(readFileSync(new URL('mongo-contract-exceptions.json', import.meta.url), 'utf8'));
  const source = readFileSync(new URL('mongo-contract-tests.sh', import.meta.url), 'utf8');
  const authenticated = readFileSync(new URL('image-manager-mongo-contract.sh', import.meta.url), 'utf8');
  assert.equal(declared[permission]?.kind, 'fixture');
  assert.ok(source.includes(`not test(=${permission})`));
  assert.ok(!source.includes('not test(store::account_deletions::tests::)'));
  assert.ok(authenticated.includes(`package(=cma) and test(=${permission})`));
  assert.equal(declared[ordinary], undefined);
  const metadata = inventory();
  const cases = metadata['rust-suites']['cma::bin/cma'].testcases;
  cases[permission] = caseMetadata(false);
  cases[ordinary] = caseMetadata(true);
  const exclusions = {...exceptions, [permission]: declared[permission]};
  assert.equal(check(metadata, exclusions).status, 0);
  cases[permission] = caseMetadata(true);
  assert.match(check(metadata, exclusions).stderr, /Selected non-ordinary contracts/);
  cases[permission] = caseMetadata(false);
  cases[ordinary] = caseMetadata(false);
  assert.match(check(metadata, exclusions).stderr, /Omitted eligible Mongo contracts/);
});
