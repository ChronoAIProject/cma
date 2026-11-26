import { cpSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const scaleTest = 'store::usage_reports::tests::scale::metrics_over_twenty_thousand_sandboxes_generate_full_cohort_and_indexed_pages';
const ownerTest = 'store::usage_reports::tests::owner_allocation::metrics_owner_allocations_full_cohort_at_400_and_20001_is_bounded_and_cursor_bound';
const auditTest = 'store::agent_log_segments::mongo_tests::audit_bounds::near_capacity_audit_keeps_source_bounds_and_limits_each_write_transaction';
const diagnosticsTest = 'store::mongo_diagnostics::mongo_tests::real_mongo_monitoring_privacy_and_overhead';
const scanTest = 'store::agent_log_telemetry::tests::cooperation::benchmark::local_scan_scheduling_abba_reproduction';
const bsonCeilingTest = 'store::sandbox_mongo_integration_tests::resource_access_tests::personal_inventory::failures::personal_agent_list_preserves_sources_near_the_bson_document_ceiling';
const firstResponseTest = 'store::usage_reports::tests::submission::default_day_report_serves_four_hundred_sandboxes_without_polling';
const historyTest = 'store::usage_reports::tests::history_batch::seven_day_history_generates_and_reuses_complete_421_member_report';
const isolatedTests = [scaleTest, ownerTest, auditTest, diagnosticsTest, scanTest, bsonCeilingTest, firstResponseTest, historyTest];
// On-demand only: neither the listing nor any PR shard selects it.
const observationTest = 'store::sandbox_mongo_integration_tests::resource_access_tests::history_measurement::observation_budget::observation_one_hundred_observers_on_two_replicas_for_thirty_minutes';
// The isolated fixtures start many mock subprocesses before ordinary work.
// Bound that setup separately from the signal-to-cleanup deadline below.
const runnerSetupTimeoutMs = 30_000;
const fixturePredicate = 'not test(=auth::account_standing::mongo_tests::replicas::fresh_snapshot_cannot_mask_real_database_permission_failure) & not test(=agent_manager::relay::tests::mongo::preferences::process_recovery::child::preferences_replica_child) & not test(=store::bindings::reset_tests::gateway_reset_tests::configured_gateway_replays_real_m1_reset_and_preserves_both_scope_floors) & not test(=store::bindings::reset_tests::gateway_reset_tests::profile_reset_receipt_replays_after_a_later_channel_revision) & not test(=store::sandbox_usage::raw_event::tests::allocation_raw_event_cpu_comparison) & not test(=store::usage_reports::tests::hotfix_cpu::native_usage_hotfix_decode_phase_calibration) & not test(=store::usage_reports::tests::hotfix_matrix::native_usage_hotfix_full_report_six_case_matrix) & not test(store::migrations::tests::segments_runtime::) & not test(=sandbox::agent_worker_tests::sdk_conformance_tests::task_home_tests::subprocess::task_home_create_child) & not test(=store::sandbox_sessions::exclusive::process_tests::desktop_claim_child) & not test(=store::sandbox_sessions::exclusive::service_tests::latency_tests::repeated_desktop_connect_windows) & not test(=store::sandbox_sessions::exclusive::service_tests::endpoint_tests::public_disabled_services_and_ownerless_receiver_refuse_new_streams) & not test(=store::sandbox_sessions::exclusive::service_tests::endpoint_tests::retained_legacy_rows_block_until_close_or_expiry_and_cannot_release_successor) & not test(=store::sandbox_sessions::exclusive::service_tests::endpoint_tests::public_replica_handoff_and_revocation_use_actual_receiver) & not test(=store::sandbox_sessions::exclusive::service_tests::endpoint_tests::public_uncertain_confirm_and_receiver_restart_preserve_fencing) & not test(=store::sandbox_sessions::exclusive::service_tests::endpoint_tests::delayed_receiver_renewal_ack_cannot_restore_public_authority) & not test(=store::sandbox_sessions::exclusive::service_tests::endpoint_tests::measurements::matched_public_desktop_connect_windows) & not test(=store::sandbox_mongo_integration_tests::command_ledger_tests::execution_tests::stale_turn_tests::native_stop_trace::native_stop_trace_recorder_ack_matches_a_fresh_checkpoint) & not test(=store::agent_log_segments::active_proof::prepare::tests::active_proof_operator_cli_uses_generated_isolated_role)';
const qualificationPredicate = `not (test(=store::sandbox_mongo_integration_tests::resource_access_tests::personal_inventory::concurrency::personal_agent_list_phase_profile) or test(=store::sandbox_mongo_integration_tests::resource_access_tests::personal_inventory::concurrency::personal_agent_provenance_phase_profile) or test(=store::sandbox_mongo_integration_tests::resource_access_tests::personal_inventory::concurrency::personal_agent_list_one_hundred_concurrent_public_readers_meet_budgets) or test(=store::sandbox_mongo_integration_tests::resource_access_tests::personal_inventory::concurrency::personal_agent_provenance_one_hundred_concurrent_public_readers_meet_budgets) or test(=store::sandbox_mongo_integration_tests::resource_access_tests::personal_inventory::availability::personal_inventory_constrained_availability_comparison) or test(=store::sandbox_mongo_integration_tests::resource_access_tests::personal_inventory::organization::organization_one_hundred_concurrent_public_readers_meet_budgets) or test(=${observationTest}))`;
const ordinaryPredicate = `not (test(=${scaleTest}) or test(=${ownerTest}) or test(=${auditTest}) or test(=${diagnosticsTest}) or test(=${scanTest}) or test(=${bsonCeilingTest}) or test(=${firstResponseTest}) or test(=${historyTest})) & ${qualificationPredicate}`;

function fixture(scenario, reuse = false) {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), 'cma-mongo-runner-test-')));
  const root = join(directory, 'repo');
  const scripts = join(root, '.github/scripts');
  mkdirSync(scripts, {recursive: true});
  mkdirSync(join(root, 'api/target/nextest'), {recursive: true});
  for (const name of ['mongo-contract-tests.sh', 'mongo-contract-registry.py', 'mongo-contract-partitions.py', 'mongo-contract-schedule.py', 'mongo-contract-timings.json', 'mongo-contract-completeness.py', 'mongo-contract-exceptions.json', 'backend-nextest.sh',
    'pull-pinned-image.sh']) {
    copyFileSync(new URL(name, import.meta.url), join(scripts, name));
  }
  cpSync(new URL('mongo-contract-suites', import.meta.url), join(scripts, 'mongo-contract-suites'), {recursive: true});
  const metadata = join(root, 'api/target/nextest');
  writeFileSync(join(metadata, 'cargo-metadata.json'), JSON.stringify({workspace_root: join(root, 'api'), target_directory: join(root, 'api/target')}));
  writeFileSync(join(metadata, 'binaries-metadata.json'), JSON.stringify({'rust-build-meta': {'target-directory': join(root, 'api/target')}}));
  const log = join(directory, 'calls.ndjson');
  const foreignVolume = `${log}.volume-foreign-retained`;
  writeFileSync(foreignVolume, JSON.stringify({owner: 'another-owner'}));
  const cli = `#!/usr/bin/env node
const fs = require('node:fs');
const path = require('node:path');
const command = path.basename(process.argv[1]);
const args = process.argv.slice(2);
const scenario = process.env.CMA_RUNNER_SCENARIO;
const log = process.env.CMA_RUNNER_LOG;
if (command === 'openssl') {
  if (scenario === 'entropy-error') process.exit(1);
  console.log(scenario === 'entropy-malformed' ? 'bad-suffix' : process.pid.toString(16).padStart(24, '0'));
  process.exit(0);
}
fs.appendFileSync(log, JSON.stringify({command,args,uri:process.env.CHRONO_TEST_MONGODB_URI,authenticatedUri:process.env.CMA_IM_TEST_MONGODB_URI,retriesEnv:process.env.NEXTEST_RETRIES,flakyResultEnv:process.env.NEXTEST_FLAKY_RESULT})+'\\n');
if (command === 'cargo') {
  if (args[1] === 'run' && args.filter(value => value === '--no-tests').length !== 1) process.exit(24);
  if (args[1] === 'list') {
    if (scenario === 'list-error') process.exit(7);
    if (scenario === 'list-malformed') { console.log('invalid-json'); process.exit(0); }
    const count = ['list-empty', 'list-no-first-response','list-first-response-not-ignored','list-duplicate-first-response','list-no-ordinary'].includes(scenario) ? 0 : scenario.startsWith('shard') ? 321 : scenario.startsWith('batches') ? 161 : 80;
    const testcases = Object.fromEntries(Array.from({length:count}, (_, index) => [
      'store::usage_reports::tests::ordinary_'+index, {ignored:scenario !== 'list-not-ignored', 'filter-match':{status:'matches'}},
    ]));
    if (!['list-empty', 'list-no-scale'].includes(scenario)) {
      testcases[${JSON.stringify(scaleTest)}] = {ignored:scenario !== 'list-scale-not-ignored', 'filter-match':{status:'matches'}};
    }
    if (!['list-empty', 'list-no-owner'].includes(scenario)) {
      testcases[${JSON.stringify(ownerTest)}] = {ignored:scenario !== 'list-owner-not-ignored', 'filter-match':{status:'matches'}};
    }
    if (!['list-empty', 'list-no-audit'].includes(scenario)) {
      testcases[${JSON.stringify(auditTest)}] = {ignored:scenario !== 'list-audit-not-ignored', 'filter-match':{status:'matches'}};
    }
    if (!['list-empty', 'list-no-diagnostics'].includes(scenario)) {
      testcases[${JSON.stringify(diagnosticsTest)}] = {ignored:scenario !== 'list-diagnostics-not-ignored', 'filter-match':{status:'matches'}};
    }
    if (!['list-empty', 'list-no-scan'].includes(scenario)) {
      testcases[${JSON.stringify(scanTest)}] = {ignored:scenario !== 'list-scan-not-ignored', 'filter-match':{status:'matches'}};
    }
    if (!['list-empty', 'list-no-bson'].includes(scenario)) {
      testcases[${JSON.stringify(bsonCeilingTest)}] = {ignored:scenario !== 'list-bson-not-ignored', 'filter-match':{status:'matches'}};
    }
    if (!['list-empty', 'list-no-first-response'].includes(scenario)) {
      testcases[${JSON.stringify(firstResponseTest)}] = {ignored:scenario !== 'list-first-response-not-ignored', 'filter-match':{status:'matches'}};
    }
    if (!['list-empty', 'list-no-history'].includes(scenario)) {
      testcases[${JSON.stringify(historyTest)}] = {ignored:scenario !== 'list-history-not-ignored', 'filter-match':{status:'matches'}};
    }
    for (const name of Object.keys(JSON.parse(fs.readFileSync(path.join(process.cwd(), '../.github/scripts/mongo-contract-exceptions.json'))))) {
      testcases[name] = {ignored:true, 'filter-match':{status:'mismatch'}};
    }
    if (scenario === 'list-omitted-module') testcases['new_module::mongo_contract'] = {ignored:true, 'filter-match':{status:'mismatch'}};
    if (scenario === 'list-selected-helper') testcases['sandbox::agent_worker_tests::sdk_conformance_tests::task_home_tests::subprocess::task_home_create_child']['filter-match'].status = 'matches';
    if (scenario === 'list-stale-exception') delete testcases['sandbox::agent_worker_tests::sdk_conformance_tests::official_responses_sdk_server'];
    const suites = {'cma::bin/cma':{'package-name':'cma', 'binary-name':'cma', kind:'bin', testcases}};
    if (scenario === 'list-duplicate-scale') suites.duplicate = {testcases:{[${JSON.stringify(scaleTest)}]:testcases[${JSON.stringify(scaleTest)}]}};
    if (scenario === 'list-duplicate-owner') suites.duplicate = {testcases:{[${JSON.stringify(ownerTest)}]:testcases[${JSON.stringify(ownerTest)}]}};
    if (scenario === 'list-duplicate-audit') suites.duplicate = {testcases:{[${JSON.stringify(auditTest)}]:testcases[${JSON.stringify(auditTest)}]}};
    if (scenario === 'list-duplicate-diagnostics') suites.duplicate = {testcases:{[${JSON.stringify(diagnosticsTest)}]:testcases[${JSON.stringify(diagnosticsTest)}]}};
    if (scenario === 'list-duplicate-scan') suites.duplicate = {testcases:{[${JSON.stringify(scanTest)}]:testcases[${JSON.stringify(scanTest)}]}};
    if (scenario === 'list-duplicate-bson') suites.duplicate = {testcases:{[${JSON.stringify(bsonCeilingTest)}]:testcases[${JSON.stringify(bsonCeilingTest)}]}};
    if (scenario === 'list-duplicate-first-response') suites.duplicate = {testcases:{[${JSON.stringify(firstResponseTest)}]:testcases[${JSON.stringify(firstResponseTest)}]}};
    if (scenario === 'list-duplicate-history') suites.duplicate = {testcases:{[${JSON.stringify(historyTest)}]:testcases[${JSON.stringify(historyTest)}]}};
    console.log(JSON.stringify({'rust-suites':suites}));
    process.exit(0);
  }
  if (args[args.indexOf('-E')+1].startsWith('package(=cma) and test(=${observationTest})')) {
    if (args.includes('--partition')) process.exit(8);
    process.exit(scenario === 'observation-error' ? 7 : 0);
  }
  if (args[args.indexOf('-E')+1].startsWith('package(=cma) and test(::cpu_profile::)')) {
    if (args.includes('--partition')) process.exit(8);
    process.exit(scenario === 'cpu-error' ? 7 : scenario === 'cpu-empty' ? 4 : 0);
  }
  const scale = args[args.indexOf('-E')+1].startsWith('package(=cma) and test(=${scaleTest})');
  const owner = args[args.indexOf('-E')+1].startsWith('package(=cma) and test(=${ownerTest})');
  const audit = args[args.indexOf('-E')+1].startsWith('package(=cma) and test(=${auditTest})');
  const diagnostics = args[args.indexOf('-E')+1].startsWith('package(=cma) and test(=${diagnosticsTest})');
  const scan = args[args.indexOf('-E')+1].startsWith('package(=cma) and test(=${scanTest})');
  const bson = args[args.indexOf('-E')+1].startsWith('package(=cma) and test(=${bsonCeilingTest})');
  const firstResponse = args[args.indexOf('-E')+1].startsWith('package(=cma) and test(=${firstResponseTest})');
  const history = args[args.indexOf('-E')+1].startsWith('package(=cma) and test(=${historyTest})');
  const isolated = history || scale || owner || audit || diagnostics || scan || bson || firstResponse;
  const partition = isolated ? 'count:1/1' : args[args.indexOf('--partition') + 1];
  const match = /^count:([1-9][0-9]*)\\/([1-9][0-9]*)$/.exec(partition);
  if (!match) process.exit(8);
  const index = scale ? 0 : owner ? -1 : audit ? -2 : diagnostics ? -3 : scan ? -4 : bson ? -5 : firstResponse ? -6 : history ? -7 : Number(match[1]);
  const count = scenario.startsWith('shard') ? 321 : scenario.startsWith('batches') ? 161 : 80;
  const selected = scale ? [${JSON.stringify(scaleTest)}] : owner ? [${JSON.stringify(ownerTest)}] : audit ? [${JSON.stringify(auditTest)}] : diagnostics ? [${JSON.stringify(diagnosticsTest)}] : scan ? [${JSON.stringify(scanTest)}] : bson ? [${JSON.stringify(bsonCeilingTest)}] : firstResponse ? [${JSON.stringify(firstResponseTest)}] : history ? [${JSON.stringify(historyTest)}] : Array.from({length:count}, (_, i) =>
    'store::usage_reports::tests::ordinary_'+i).filter((_, i) => i % Number(match[2]) === index-1);
  fs.writeFileSync(log+'.selected-'+index, JSON.stringify(selected));
  fs.writeFileSync(log+'.started-'+index, 'started');
  if (scenario === 'batches-cleanup-error' && index === 1) fs.writeFileSync(log+'.failed-cleanup-uri', process.env.CHRONO_TEST_MONGODB_URI);
  if ((scenario === 'scale-empty' && scale) || (scenario === 'owner-empty' && owner) || (scenario === 'audit-empty' && audit) || (scenario === 'diagnostics-empty' && diagnostics) || (scenario === 'scan-empty' && scan) || (scenario === 'bson-empty' && bson) || (scenario === 'first-response-empty' && firstResponse) || (scenario === 'history-empty' && history) || (scenario === 'ordinary-empty' && index === 1)) {
    console.log('Starting 0 tests');
    const option = args.indexOf('--no-tests');
    const policy = option >= 0 ? args[option+1] : process.env.NEXTEST_NO_TESTS;
    fs.writeFileSync(log+'.empty-'+index, policy || 'auto');
    process.exit(policy === 'pass' ? 0 : 4);
  }
  if ((scenario === 'batches-cleanup-error' && index === 2) || (scenario === 'cancel' && !isolated) || (scenario === 'cancel-scale' && scale) || (scenario === 'cancel-owner' && owner) || (scenario === 'cancel-audit' && audit) || (scenario === 'cancel-diagnostics' && diagnostics) || (scenario === 'cancel-scan' && scan) || (scenario === 'cancel-history' && history)) { setInterval(() => {}, 1000); }
  else {
    const deadline = Date.now()+5000;
    while (!scenario.startsWith('shard') && index >= 1 && index <= 2 && !fs.existsSync(log+'.started-'+(3-index))) {
      if (Date.now() >= deadline) process.exit(9);
      Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 10);
    }
    fs.writeFileSync(log+'.finished-'+index, 'finished');
    process.exit((['test-error','batches-error','shard-error'].includes(scenario) && index === 1) || (scenario === 'scale-error' && scale) || (scenario === 'owner-error' && owner) || (scenario === 'audit-error' && audit) || (scenario === 'diagnostics-error' && diagnostics) || (scenario === 'scan-error' && scan) || (scenario === 'bson-error' && bson) || (scenario === 'first-response-error' && firstResponse) || (scenario === 'history-error' && history) ? 7 : 0);
  }
} else {
  const file = name => log+'.container-'+name;
  const volume = name => log+'.volume-'+name;
  if (args[0] === 'volume') {
    if (args[1] === 'ls') {
      if (scenario === 'volume-list-error') process.exit(16);
      const filter = args[args.indexOf('--filter')+1];
      if (!/^name=\\^cma-mongo-ci-(?:predecessor-)?[0-9a-f]{24}-data\\$$/.test(filter)) process.exit(17);
      const name = filter.slice(6,-1);
      if (scenario === 'foreign-volume') fs.writeFileSync(volume(name), JSON.stringify({owner:'another-owner'}));
      console.log(fs.existsSync(volume(name)) ? name : '');
    } else if (args[1] === 'create') {
      const name = args.at(-1);
      if (fs.existsSync(volume(name))) process.exit(18);
      const owner = args[args.indexOf('--label')+1].split('=').at(-1);
      fs.writeFileSync(volume(name), JSON.stringify({owner:scenario === 'volume-owner-mismatch' ? 'another-owner' : owner}));
      if (scenario === 'volume-create-error') process.exit(19);
      console.log(name);
    } else if (args[1] === 'inspect') {
      console.log(JSON.parse(fs.readFileSync(volume(args.at(-1)))).owner);
    } else if (args[1] === 'rm') {
      const name = args[2];
      const owner = JSON.parse(fs.readFileSync(volume(name))).owner;
      if (name !== owner+'-data' || args.length !== 3) process.exit(20);
      if (scenario === 'volume-cleanup-error' || (scenario === 'owner-volume-cleanup-error' && fs.existsSync(log+'.finished--1')) || (scenario === 'audit-volume-cleanup-error' && fs.existsSync(log+'.finished--2')) || (scenario === 'diagnostics-volume-cleanup-error' && fs.existsSync(log+'.finished--3')) || (scenario === 'scan-volume-cleanup-error' && fs.existsSync(log+'.finished--4')) || (scenario === 'history-volume-cleanup-error' && fs.existsSync(log+'.finished--7')) || fs.existsSync(file(owner))) process.exit(21);
      fs.unlinkSync(volume(name));
    } else process.exit(22);
  } else if (args[0] === 'run') {
    if (scenario === 'run-error') process.exit(23);
    const name = args[args.indexOf('--name')+1];
    let port=32777;
    while (true) {
      try { fs.mkdirSync(log+'.port-'+port); break; }
      catch (error) { if (error.code !== 'EEXIST') throw error; port++; }
    }
    const id = require('node:crypto').createHash('sha256').update(name).digest('hex');
    fs.writeFileSync(file(name), JSON.stringify({id,port})); console.log(id);
  } else if (args[0] === 'exec') {
    const phase = args.at(-1).includes('ping') ? 'ping' : args.at(-1).includes('isWritablePrimary') ? 'primary' : 'init';
    const marker = log+'.ready-'+phase;
    if (scenario === 'readiness-retry' && phase !== 'init' && !fs.existsSync(marker)) {
      fs.writeFileSync(marker, 'attempted'); process.exit(1);
    }
    if (phase === 'primary' && args.at(-1).includes('topologyVersion')) {
      const id = require('node:crypto').createHash('sha256').update(args[1]).digest('hex').slice(0, 24);
      console.log(id + ' ' + Date.now());
    } else console.log('1');
  }
  else if (args[0] === 'port') console.log('127.0.0.1:'+JSON.parse(fs.readFileSync(file(args[1]))).port);
  else if (args[0] === 'ps') {
    const name = args.find(value => value.startsWith('label=')).split('=').at(-1);
    console.log(fs.existsSync(file(name)) ? JSON.parse(fs.readFileSync(file(name))).id.slice(0,12) : '');
  } else if (args[0] === 'rm') {
    if (scenario === 'cleanup-error' || (scenario === 'owner-cleanup-error' && fs.existsSync(log+'.finished--1')) || (scenario === 'audit-cleanup-error' && fs.existsSync(log+'.finished--2')) || (scenario === 'diagnostics-cleanup-error' && fs.existsSync(log+'.finished--3')) || (scenario === 'scan-cleanup-error' && fs.existsSync(log+'.finished--4')) || (scenario === 'history-cleanup-error' && fs.existsSync(log+'.finished--7'))) process.exit(1);
    const files = fs.readdirSync(path.dirname(log)).filter(name => name.startsWith('calls.ndjson.container-'));
    // Match immutable mock identity without reading another concurrently removed file.
    const selected = files.find(name => require('node:crypto').createHash('sha256')
      .update(name.slice('calls.ndjson.container-'.length)).digest('hex').startsWith(args[2]));
    if (!selected) process.exit(10);
    if (scenario === 'batches-cleanup-error' && fs.existsSync(log+'.failed-cleanup-uri') && fs.readFileSync(log+'.failed-cleanup-uri', 'utf8').includes(':'+JSON.parse(fs.readFileSync(path.join(path.dirname(log),selected))).port+'/')) process.exit(1);
    fs.unlinkSync(path.join(path.dirname(log),selected));
  } else if (args[0] === 'image' && args[1] === 'inspect') {
    // The pinned fixture image is present; pull-pinned-image.sh's own tests cover pulls.
    process.exit(0);
  } else process.exit(11);
}
`;
  for (const name of ['openssl', 'docker', 'cargo']) writeFileSync(join(directory, name), cli, { mode: 0o755 });
  const env = {
    ...process.env, CI: reuse ? 'true' : 'false', PATH: `${directory}:${process.env.PATH}`, CMA_RUNNER_SCENARIO: scenario, CMA_RUNNER_LOG: log,
    CHRONO_TEST_MONGODB_URI: 'must-not-use-caller-uri', CHRONO_TEST_MONGODB_FAILPOINT_URI: 'must-not-use-caller-uri',
    CMA_IM_TEST_MONGODB_URI: 'must-not-use-authenticated-caller-uri',
  };
  if (reuse) env.CMA_NEXTEST_REUSE = '1'; else delete env.CMA_NEXTEST_REUSE;
  return {
    directory, log, env, foreignVolume, metadata,
    args: [join(scripts, 'mongo-contract-tests.sh')],
    calls: () => readFileSync(log, 'utf8').trim().split('\n').map(line => JSON.parse(line)),
    containers: () => readdirSync(directory).filter(name => name.startsWith('calls.ndjson.container-')),
    volumes: () => readdirSync(directory).filter(name => /^calls\.ndjson\.volume-cma-mongo-ci-/.test(name)),
    close: () => rmSync(directory, { recursive: true }),
  };
}

export { scaleTest, ownerTest, auditTest, diagnosticsTest, scanTest, bsonCeilingTest, firstResponseTest, historyTest, isolatedTests, observationTest, runnerSetupTimeoutMs, fixturePredicate, qualificationPredicate, ordinaryPredicate, fixture };
