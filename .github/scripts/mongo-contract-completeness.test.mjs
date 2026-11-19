import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
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
    'agent_lifecycle::workspace::tests::quota::quota_preflight_preserves_replay_and_existing_targets_without_preparation',
    'agent_lifecycle::workspace::tests::quota::worker_quota_failure_metadata_is_gated_and_survives_restart',
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
  assert.equal(new Set(requiredPrefixes).size, 145);
  assert.ok(requiredPrefixes.includes('store::directory::mongo_tests::'));
  assert.ok(requiredPrefixes.includes('sandbox::agent_worker_tests::cleanup_contention_tests::'));
  assert.ok(requiredPrefixes.includes('store::agent_lifecycle_operations::lease_tests::'));
  assert.ok(requiredPrefixes.includes('agent_manager::relay::tests::mongo::'));
  assert.ok(requiredPrefixes.includes('store::agent_ownership::mongo_tests::'));
  assert.ok(requiredPrefixes.includes('auth::desktop::tests::'));
  assert.ok(requiredPrefixes.includes('store::sandbox_sessions::exclusive::'));
  assert.ok(requiredPrefixes.includes('store::allocation_buckets::'));
  assert.ok(requiredPrefixes.includes(taskHome));
  assert.ok(requiredPrefixes.includes('agent_lifecycle::workspace::tests::quota::'));
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
