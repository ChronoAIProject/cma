import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { requiredPrefixes } from './mongo-contract-test-prefixes.mjs';

const taskHome = 'sandbox::agent_worker_tests::sdk_conformance_tests::task_home_tests::';
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
`, new URL('mongo-contract-completeness.py', import.meta.url).pathname], {
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
    `${child}_another_contract`,
  ]) {
    const metadata = inventory();
    metadata['rust-suites']['cma::bin/cma'].testcases[name] = caseMetadata(false);
    const result = check(metadata);
    assert.equal(result.status, 1);
    assert.ok(result.stderr.includes(`Omitted eligible Mongo contracts:\n  ${name}`), result.stderr);
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

test('startup SIGTERM helper is classified by exact name without hiding sibling contracts', () => {
  const startupChild = 'shutdown::signals::tests::termination_during_startup_child';
  const declared = JSON.parse(readFileSync(new URL('mongo-contract-exceptions.json', import.meta.url), 'utf8'));
  assert.equal(declared[startupChild].kind, 'helper');
  const metadata = inventory();
  const cases = metadata['rust-suites']['cma::bin/cma'].testcases;
  cases[startupChild] = caseMetadata(false);
  const exclusions = {...exceptions, [startupChild]: declared[startupChild]};
  assert.equal(check(metadata, exclusions).status, 0);
  assert.ok(check(metadata).stderr.includes(startupChild));
  cases[startupChild] = caseMetadata(true);
  assert.match(check(metadata, exclusions).stderr, /Selected non-ordinary contracts/);
  cases[startupChild] = caseMetadata(false);
  cases[`${startupChild}_another_contract`] = caseMetadata(false);
  assert.ok(check(metadata, exclusions).stderr.includes(`${startupChild}_another_contract`));
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

test('prefix inventory includes task-home and agrees with the runner without duplicates', () => {
  const source = readFileSync(new URL('mongo-contract-tests.sh', import.meta.url), 'utf8');
  const line = source.split('\n').find(line => line.trimStart().startsWith('-E '));
  const actual = [...line.matchAll(/(?:'|\| )test\(([^)]+)\)/g)].map(match => match[1]);
  assert.deepEqual(actual, requiredPrefixes);
  assert.equal(new Set(requiredPrefixes).size, 139);
  assert.ok(requiredPrefixes.includes('sandbox::agent_worker_tests::cleanup_contention_tests::'));
  assert.ok(requiredPrefixes.includes('store::agent_lifecycle_operations::lease_tests::'));
  assert.ok(requiredPrefixes.includes('agent_manager::relay::tests::mongo::'));
  assert.ok(requiredPrefixes.includes('store::agent_ownership::mongo_tests::'));
  assert.ok(requiredPrefixes.includes('auth::desktop::tests::'));
  assert.ok(requiredPrefixes.includes('store::sandbox_sessions::exclusive::'));
  assert.ok(requiredPrefixes.includes('store::allocation_buckets::'));
  assert.ok(requiredPrefixes.includes(taskHome));
  assert.ok(requiredPrefixes.includes('store::migrations::index_toolkit::tests::'));
  assert.ok(requiredPrefixes.includes('store::sandbox_usage::interval_index::tests::'));
  assert.ok(requiredPrefixes.includes('rate_limit::lease::mongo_tests::'));
  assert.ok(requiredPrefixes.includes('store::worker_observations::parked::tests::'));
  assert.ok(line.includes(`not test(=${child})`));
});

const desktopChild = 'store::sandbox_sessions::exclusive::process_tests::desktop_claim_child';
const desktopParent = 'store::sandbox_sessions::exclusive::process_tests::independent_processes_preserve_claim_after_holder_exit';
const desktopPerformance = 'store::sandbox_sessions::exclusive::service_tests::latency_tests::repeated_desktop_connect_windows';
const desktopEndpoints = [
  'store::sandbox_sessions::exclusive::service_tests::endpoint_tests::public_legacy_provider_connects_before_receiver_activation',
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
  assert.match(result.stdout, /11 ignored CMA tests; 3 selected; 6 separate-fixture tests; 2 child helpers/);
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


test('owner admission and queued mutation Mongo contracts remain ordinary selected tests', () => {
  const source = readFileSync(new URL('mongo-contract-tests.sh', import.meta.url), 'utf8');
  const line = source.split('\n').find(line => line.trimStart().startsWith('-E '));
  const selectedPrefixes = [...line.matchAll(/(?:'|\| )test\(([^)]+)\)/g)].map(match => match[1]);
  const declared = JSON.parse(readFileSync(new URL('mongo-contract-exceptions.json', import.meta.url), 'utf8'));
  const names = [
    'agent_manager::relay::admission::mongo_tests::real_mongo_peer_admission_commits_once_and_refuses_revoked_owner',
    'store::agent_commands::reserve::ownership_tests::admission_owner_fence_and_effect_roll_back_in_the_same_transaction',
    'store::agent_commands::reserve::ownership_tests::coordinated_admission_requires_exact_current_local_owner_and_preserves_replay',
    'store::agent_commands::reserve::ownership_tests::noncoordinated_admission_remains_available_without_owner_scope',
    'store::agent_commands::reserve::ownership_tests::pair::interruption_pair_fences_local_owner_before_either_half_and_on_replay',
    'store::agent_commands::reserve::ownership_tests::recovery::committed_edit_lookup_after_owner_handoff_is_exact_and_never_writes',
    'store::agent_commands::targeted_control::ownership_tests::queued_cancel_accepts_renewable_and_prepared_parents_with_owner_fences',
    'store::agent_commands::targeted_control::ownership_tests::queued_cancel_and_replay_require_current_durable_owner',
    'store::agent_commands::targeted_control::ownership_tests::queued_edit_accepts_renewable_and_prepared_parents_with_owner_fences',
    'store::agent_commands::targeted_control::ownership_tests::queued_edit_and_replay_require_current_durable_owner',
    'store::agent_commands::targeted_control::ownership_tests::targeted_admission_and_replay_require_owner_while_lookup_does_not_write',
  ];
  const metadata = inventory();
  const cases = metadata['rust-suites']['cma::bin/cma'].testcases;
  for (const name of names) {
    assert.equal(declared[name], undefined, name);
    assert.ok(!source.includes(`not test(=${name})`), name);
    cases[name] = caseMetadata(selectedPrefixes.some(prefix => name.startsWith(prefix)));
  }
  const result = check(metadata);
  assert.equal(result.status, 0, result.stderr);
  for (const name of names) {
    cases[name] = caseMetadata(false);
    const omitted = check(metadata);
    assert.equal(omitted.status, 1, name);
    assert.ok(omitted.stderr.includes(`Omitted eligible Mongo contracts:\n  ${name}`), omitted.stderr);
    cases[name] = caseMetadata(true);
  }
});
