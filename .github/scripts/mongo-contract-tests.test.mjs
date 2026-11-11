import assert from 'node:assert/strict';
import test from 'node:test';
import { execFileSync, spawn } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const scaleTest = 'store::usage_reports::tests::scale::metrics_over_twenty_thousand_sandboxes_generate_full_cohort_and_indexed_pages';
const ownerTest = 'store::usage_reports::tests::owner_allocation::metrics_owner_allocations_full_cohort_at_400_and_20001_is_bounded_and_cursor_bound';
const ordinaryPredicate = `not (test(=${scaleTest}) or test(=${ownerTest}))`;
const requiredPrefixes = [
  'store::usage_reports::tests::',
  'sandbox::funding::tests::mongo::',
  'usage_billing::tests::',
  'store::request_limits::tests::',
  'store::sandbox_mongo_integration_tests::sandbox_usage_tests::',
  'document_artifacts::mongo_integration_tests::',
  'store::message_feedback::tests::',
  'store::sandbox_colors::tests::',
  'agent_lifecycle::workspace::tests::mongo::',
  'agent_lifecycle::workspace::tests::bootstrap::',
  'warm_pools::tests::mongo::',
  'store::native_warm_pools::retirement::tests::',
  'store::native_prepared::tests::',
  'store::sandbox_admission_mongo_integration_tests::warmup::',
  'store::auth::api_bearer_tests::',
  'store::agent_conversation_store::checkpoint_tests::',
  'store::agent_conversation_store::admission_tests::',
  'store::agent_conversation_store::reopen_tests::',
  'store::agent_access_tools::',
  'store::agent_access_requests::',
  'agent_access_worker::',
  'owner_service_access::',
  'store::sandbox_mongo_integration_tests::warm_hierarchy_tests::',
  'store::agent_admission::mongo_integration_tests::',
  'store::schema_mongo_integration_tests::',
  'store::migrations::tests::',
  'store::sandbox_mongo_integration_tests::ledger_retirement_tests::',
  'store::workspaces::mongo_integration_tests::',
  'store::bindings::',
  'store::gateway_records_mongo_integration_tests::',
  'store::bots_mongo_integration_tests::',
  'store::bot_setups::mongo_integration_tests::',
  'bot_setups::events::tests::',
  'store::trigger_definitions::launch_mongo_integration_tests::',
  'store::trigger_definitions::archive_mongo_tests::',
  'store::checkpoint_objects::mongo_integration_tests::',
  'store::sandbox_mongo_integration_tests::checkpoint_capture_tests::',
  'store::sandbox_mongo_integration_tests::service_access_tests::',
  'store::sandbox_mongo_integration_tests::command_ledger_tests::',
  'store::sandbox_mongo_integration_tests::resource_access_tests::',
  'store::sandbox_maintenance::mongo_integration_tests::',
  'store::sandbox_maintenance::consented::scope_tests::',
  'store::sandbox_reconciliation::mongo_integration_tests::channel_home_retention::',
  'sandbox::agent_worker_tests::sdk_conformance_tests::mongo_tests::',
  'sandbox::agent_worker_tests::first_message_contention_tests::',
  'sandbox::worker_tests::lifecycle_queue_contention_tests::',
  'store::sandbox_mongo_integration_tests::sandbox_lifecycle_tests::',
  'sandbox::agent_worker::prepared_open_ready::',
  'store::mongo_latency::tests::',
  'store::agent_log_telemetry::tests::',
  'store::agent_log_integrity::quarantine_tests::',
  'store::agent_log::receipt_tests::',
  'store::mongo_read_telemetry::cursor_tests::',
  'store::sandbox_mongo_integration_tests::read_telemetry_tests::',
  'store::agent_task_status::batch::tests::',
  'store::sandbox_mongo_integration_tests::agent_worker_tests::',
  'store::sandbox_mongo_integration_tests::agent_transaction_tests::',
  'store::sandbox_mongo_integration_tests::agent_log_read_tests::',
  'store::sandbox_mongo_integration_tests::log_integrity_tests::',
  'store::sandbox_mongo_integration_tests::agent_title_tests::',
  'sandbox::agent_worker_tests::connection_failure_tests::capacity_cleanup_tests::',
];

function fixture(scenario, reuse = false) {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), 'cma-mongo-runner-test-')));
  const root = join(directory, 'repo');
  const scripts = join(root, '.github/scripts');
  mkdirSync(scripts, {recursive: true});
  mkdirSync(join(root, 'api/target/nextest'), {recursive: true});
  for (const name of ['mongo-contract-tests.sh', 'mongo-contract-partitions.py', 'backend-nextest.sh']) {
    copyFileSync(new URL(name, import.meta.url), join(scripts, name));
  }
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
fs.appendFileSync(log, JSON.stringify({command,args,uri:process.env.CHRONO_TEST_MONGODB_URI})+'\\n');
if (command === 'cargo') {
  if (args[1] === 'run' && args.filter(value => value === '--no-tests').length !== 1) process.exit(24);
  if (args[1] === 'list') {
    if (scenario === 'list-error') process.exit(7);
    if (scenario === 'list-malformed') { console.log('invalid-json'); process.exit(0); }
    const count = ['list-empty', 'list-no-ordinary'].includes(scenario) ? 0 : scenario.startsWith('batches') ? 161 : 80;
    const testcases = Object.fromEntries(Array.from({length:count}, (_, index) => [
      'store::usage_reports::tests::ordinary_'+index, {ignored:scenario !== 'list-not-ignored', 'filter-match':{status:'matches'}},
    ]));
    if (!['list-empty', 'list-no-scale'].includes(scenario)) {
      testcases[${JSON.stringify(scaleTest)}] = {ignored:scenario !== 'list-scale-not-ignored', 'filter-match':{status:'matches'}};
    }
    if (!['list-empty', 'list-no-owner'].includes(scenario)) {
      testcases[${JSON.stringify(ownerTest)}] = {ignored:scenario !== 'list-owner-not-ignored', 'filter-match':{status:'matches'}};
    }
    testcases.filtered = {ignored:true, 'filter-match':{status:'mismatch'}};
    const suites = {fixture:{testcases}};
    if (scenario === 'list-duplicate-scale') suites.duplicate = {testcases:{[${JSON.stringify(scaleTest)}]:testcases[${JSON.stringify(scaleTest)}]}};
    if (scenario === 'list-duplicate-owner') suites.duplicate = {testcases:{[${JSON.stringify(ownerTest)}]:testcases[${JSON.stringify(ownerTest)}]}};
    console.log(JSON.stringify({'rust-suites':suites}));
    process.exit(0);
  }
  if (args[args.indexOf('-E')+1].startsWith('package(=cma) and test(::cpu_profile::)')) {
    if (args.includes('--partition')) process.exit(8);
    process.exit(scenario === 'cpu-error' ? 7 : scenario === 'cpu-empty' ? 4 : 0);
  }
  const scale = args[args.indexOf('-E')+1].startsWith('package(=cma) and test(=${scaleTest})');
  const owner = args[args.indexOf('-E')+1].startsWith('package(=cma) and test(=${ownerTest})');
  const isolated = scale || owner;
  const partition = isolated ? 'count:1/1' : args[args.indexOf('--partition') + 1];
  const match = /^count:([1-9][0-9]*)\\/([1-9][0-9]*)$/.exec(partition);
  if (!match) process.exit(8);
  const index = scale ? 0 : owner ? -1 : Number(match[1]);
  const count = scenario.startsWith('batches') ? 161 : 80;
  const selected = scale ? [${JSON.stringify(scaleTest)}] : owner ? [${JSON.stringify(ownerTest)}] : Array.from({length:count}, (_, i) =>
    'store::usage_reports::tests::ordinary_'+i).filter((_, i) => i % Number(match[2]) === index-1);
  fs.writeFileSync(log+'.selected-'+index, JSON.stringify(selected));
  fs.writeFileSync(log+'.started-'+index, 'started');
  if (scenario === 'batches-cleanup-error' && index === 1) fs.writeFileSync(log+'.failed-cleanup-uri', process.env.CHRONO_TEST_MONGODB_URI);
  if ((scenario === 'scale-empty' && scale) || (scenario === 'owner-empty' && owner) || (scenario === 'ordinary-empty' && index === 1)) {
    console.log('Starting 0 tests');
    const option = args.indexOf('--no-tests');
    const policy = option >= 0 ? args[option+1] : process.env.NEXTEST_NO_TESTS;
    fs.writeFileSync(log+'.empty-'+index, policy || 'auto');
    process.exit(policy === 'pass' ? 0 : 4);
  }
  if ((scenario === 'batches-cleanup-error' && index === 2) || (scenario === 'cancel' && !isolated) || (scenario === 'cancel-scale' && scale) || (scenario === 'cancel-owner' && owner)) { setInterval(() => {}, 1000); }
  else {
    const deadline = Date.now()+5000;
    while (index >= 1 && index <= 2 && !fs.existsSync(log+'.started-'+(3-index))) {
      if (Date.now() >= deadline) process.exit(9);
      Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 10);
    }
    fs.writeFileSync(log+'.finished-'+index, 'finished');
    process.exit((['test-error','batches-error'].includes(scenario) && index === 1) || (scenario === 'scale-error' && scale) || (scenario === 'owner-error' && owner) ? 7 : 0);
  }
} else {
  const file = name => log+'.container-'+name;
  const volume = name => log+'.volume-'+name;
  if (args[0] === 'volume') {
    if (args[1] === 'ls') {
      if (scenario === 'volume-list-error') process.exit(16);
      const filter = args[args.indexOf('--filter')+1];
      if (!/^name=\\^cma-mongo-ci-[0-9a-f]{24}-data\\$$/.test(filter)) process.exit(17);
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
      if (scenario === 'volume-cleanup-error' || (scenario === 'owner-volume-cleanup-error' && fs.existsSync(log+'.finished--1')) || fs.existsSync(file(owner))) process.exit(21);
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
  } else if (args[0] === 'exec') console.log('1');
  else if (args[0] === 'port') console.log('127.0.0.1:'+JSON.parse(fs.readFileSync(file(args[1]))).port);
  else if (args[0] === 'ps') {
    const name = args.find(value => value.startsWith('label=')).split('=').at(-1);
    console.log(fs.existsSync(file(name)) ? JSON.parse(fs.readFileSync(file(name))).id.slice(0,12) : '');
  } else if (args[0] === 'rm') {
    if (scenario === 'cleanup-error' || (scenario === 'owner-cleanup-error' && fs.existsSync(log+'.finished--1'))) process.exit(1);
    const files = fs.readdirSync(path.dirname(log)).filter(name => name.startsWith('calls.ndjson.container-'));
    // Match immutable mock identity without reading another concurrently removed file.
    const selected = files.find(name => require('node:crypto').createHash('sha256')
      .update(name.slice('calls.ndjson.container-'.length)).digest('hex').startsWith(args[2]));
    if (!selected) process.exit(10);
    if (scenario === 'batches-cleanup-error' && fs.existsSync(log+'.failed-cleanup-uri') && fs.readFileSync(log+'.failed-cleanup-uri', 'utf8').includes(':'+JSON.parse(fs.readFileSync(path.join(path.dirname(log),selected))).port+'/')) process.exit(1);
    fs.unlinkSync(path.join(path.dirname(log),selected));
  } else process.exit(11);
}
`;
  for (const name of ['openssl', 'docker', 'cargo']) writeFileSync(join(directory, name), cli, { mode: 0o755 });
  const env = {
    ...process.env, CI: reuse ? 'true' : 'false', PATH: `${directory}:${process.env.PATH}`, CMA_RUNNER_SCENARIO: scenario, CMA_RUNNER_LOG: log,
    CHRONO_TEST_MONGODB_URI: 'must-not-use-caller-uri', CHRONO_TEST_MONGODB_FAILPOINT_URI: 'must-not-use-caller-uri',
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

for (const reuse of [false, true]) {
  const mode = reuse ? 'archive' : 'source';
  test(`${mode}: CPU profiles retain the shared runner, selection and failure policy`, () => {
    for (const scenario of ['success', 'cpu-error', 'cpu-empty']) {
      const f = fixture(scenario, reuse);
      try {
        const run = () => execFileSync('bash', [...f.args, '--cpu-profile'], {env: f.env, timeout: 15000, stdio: 'pipe'});
        if (scenario === 'success') run(); else assert.throws(run, error => error.status === (scenario === 'cpu-error' ? 7 : 4));
        const cargo = f.calls().filter(row => row.command === 'cargo');
        assert.equal(cargo.length, 1);
        const build = reuse ? ['--binaries-metadata', join(f.metadata, 'binaries-metadata.json'), '--cargo-metadata', join(f.metadata, 'cargo-metadata.json')] : ['--workspace', '--locked'];
        const selection = `package(=cma) and test(::cpu_profile::) & (${requiredPrefixes.map(prefix => `test(${prefix})`).join(' | ')})`;
        assert.deepEqual(cargo[0].args, ['nextest', 'run', ...build, '--no-tests', 'fail', '--test-threads', '1', '--no-fail-fast', '--success-output', 'final', '--run-ignored', 'only', '-E', selection]);
        assert.match(cargo[0].uri, /^mongodb:\/\/127\.0\.0\.1:[0-9]+\/\?directConnection=true&replicaSet=cma_ci$/);
        assert.equal(f.calls().filter(row => row.command === 'docker' && row.args[0] === 'run').length, 1);
        assert.equal(f.containers().length, 0);
        assert.equal(f.volumes().length, 0);
        assert.ok(existsSync(f.foreignVolume));
      } finally { f.close(); }
    }
  });

  test(`${mode}: report scale and owner allocation run alone before concurrent Mongo partitions, preserves failures and removes only owned fixtures`, () => {
    for (const scenario of ['entropy-error', 'entropy-malformed', 'success', 'scale-error', 'owner-error', 'test-error']) {
      const f = fixture(scenario, reuse);
      try {
        const run = () => execFileSync('bash', f.args, { env: f.env, timeout: 15000, stdio: 'pipe' });
        if (scenario === 'success') run(); else assert.throws(run);
        if (scenario.startsWith('entropy-')) { assert.deepEqual(f.calls().map(row => [row.command,row.args[1]]), [['cargo','list']]); continue; }
        const calls = f.calls();
        const cargo = calls.filter(row => row.command === 'cargo' && row.args[1] === 'run');
        for (const row of calls.filter(row => row.command === 'cargo')) {
          const action = row.args[1];
          const predicate = action === 'list' ? 'all()' : row === cargo[0] ? `package(=cma) and test(=${scaleTest})` : row === cargo[1] ? `package(=cma) and test(=${ownerTest})` : ordinaryPredicate;
          const selection = `${predicate} & (${requiredPrefixes.map(prefix => `test(${prefix})`).join(' | ')})`;
          const build = reuse ? ['--binaries-metadata', join(f.metadata, 'binaries-metadata.json'), '--cargo-metadata', join(f.metadata, 'cargo-metadata.json')] : ['--workspace', '--locked'];
          const operation = action === 'list' ? ['--message-format', 'json'] : ['--no-tests', 'fail', '--test-threads', '1', '--no-fail-fast', ...(cargo.indexOf(row) < 2 ? ['--success-output', 'final'] : ['--partition', row.args[row.args.indexOf('--partition')+1]])];
          assert.deepEqual(row.args, ['nextest', action, ...build, ...operation, '--run-ignored', 'only', '-E', selection]);
          if (action === 'list') assert.equal(row.uri, undefined, 'listing must discard caller Mongo URI too');
        }

        assert.equal(cargo.length, 4);
        assert.ok(cargo.slice(0, 2).every(row => !row.args.includes('--partition')));
        assert.deepEqual(cargo.slice(2).map(row => row.args[row.args.indexOf('--partition') + 1]).sort(), ['count:1/2', 'count:2/2']);
        assert.equal(new Set(cargo.map(row => row.uri)).size, 4);
        for (const row of cargo) {
          assert.match(row.uri, /^mongodb:\/\/127\.0\.0\.1:[0-9]+\/\?directConnection=true&replicaSet=cma_ci$/);
          assert.ok(row.args.includes('--run-ignored') && row.args.includes('only'));
          assert.ok(row.args.includes('--no-fail-fast'));
          assert.equal(row.args[row.args.indexOf('--test-threads') + 1], '1');
        }
        for (const index of [0, -1, 1, 2]) assert.ok(existsSync(`${f.log}.finished-${index}`), 'both partitions must finish, including after one fails');
        const scale = JSON.parse(readFileSync(`${f.log}.selected-0`));
        const owner = JSON.parse(readFileSync(`${f.log}.selected--1`));
        const ordinary = [1, 2].flatMap(index => JSON.parse(readFileSync(`${f.log}.selected-${index}`)));
        assert.deepEqual(scale, [scaleTest]);
        assert.deepEqual(owner, [ownerTest]);
        assert.equal(ordinary.length, 80);
        assert.equal(new Set([...scale, ...owner, ...ordinary]).size, 82, 'both isolated phases plus ordinary partitions select every mock test exactly once');
        const created = calls.filter(row => row.args[0] === 'run').map(row => row.args[row.args.indexOf('--name') + 1]);
        assert.equal(new Set(created).size, 4);
        for (const index of [0, 1]) {
          const cleanup = calls.findIndex(row => row.args[0] === 'volume' && row.args[1] === 'rm' && row.args[2] === `${created[index]}-data`);
          const nextStart = calls.findIndex(row => row.args[0] === 'run' && row.args.includes(created[index+1]));
          assert.ok(cleanup >= 0 && nextStart > cleanup, 'each exclusive fixture is removed before the next phase starts');
        }
        assert.ok(created.every(name => /^cma-mongo-ci-[0-9a-f]{24}$/.test(name)));
        const volumeCreates = calls.filter(row => row.args[0] === 'volume' && row.args[1] === 'create');
        assert.equal(volumeCreates.length, 4);
        for (const name of created) {
          assert.deepEqual(volumeCreates.find(row => row.args.at(-1) === `${name}-data`).args,
            ['volume', 'create', '--label', `dev.chrono.cma.mongo-contract=${name}`, `${name}-data`]);
          const run = calls.find(row => row.args[0] === 'run' && row.args.includes(name));
          assert.equal(run.args[run.args.indexOf('--mount') + 1], `type=volume,source=${name}-data,target=/data/db`);
          assert.equal(run.args[run.args.indexOf('--tmpfs') + 1], '/data/configdb:rw,size=16777216');
          assert.equal(run.args.filter(value => value === '--tmpfs').length, 1);
          assert.equal(run.args[run.args.indexOf('--memory') + 1], '2g');
          assert.equal(run.args[run.args.indexOf('--memory-swap') + 1], '2g');
          assert.equal(run.args[run.args.indexOf('--wiredTigerCacheSizeGB') + 1], '0.25');
          const removedVolume = calls.find(row => row.args[0] === 'volume' && row.args[1] === 'rm' && row.args[2] === `${name}-data`);
          assert.deepEqual(removedVolume.args, ['volume', 'rm', `${name}-data`]);
          assert.ok(calls.slice(0, calls.indexOf(removedVolume)).some(row => row.args[0] === 'volume' && row.args[1] === 'inspect' && row.args.at(-1) === `${name}-data`));
        }
        assert.ok(calls.filter(row => row.args[0] === 'ps').every(row => created.some(name => row.args.includes(`label=dev.chrono.cma.mongo-contract=${name}`))));
        const removed = calls.filter(row => row.args[0] === 'rm');
        assert.equal(removed.length, 4);
        assert.ok(removed.every(row => row.args.length === 3 && row.args[1] === '--force' && /^[0-9a-f]{12}$/.test(row.args[2])));
        assert.equal(f.containers().length, 0);
        assert.equal(f.volumes().length, 0);
        assert.ok(existsSync(f.foreignVolume));
      } finally { f.close(); }
    }
  });

  test(`${mode}: uncertain exclusive cleanup stops before another fixture can overlap`, () => {
    for (const scenario of ['cleanup-error', 'volume-cleanup-error', 'owner-cleanup-error', 'owner-volume-cleanup-error']) {
      const f = fixture(scenario, reuse);
      try {
        assert.throws(() => execFileSync('bash', f.args, {env: f.env, timeout: 15000, stdio: 'pipe'}), error => error.status === 1);
        const created = f.calls().filter(row => row.command === 'docker' && row.args[0] === 'run');
        assert.equal(created.length, scenario.startsWith('owner-') ? 2 : 1);
        assert.equal(existsSync(`${f.log}.started-1`), false, 'ordinary work cannot start while exclusive cleanup is uncertain');
        assert.equal(f.containers().length, scenario.includes('volume') ? 0 : 1);
        assert.equal(f.volumes().length, 1);
        assert.ok(existsSync(f.foreignVolume));
      } finally { f.close(); }
    }
  });

  test(`${mode}: ordinary cleanup failure stops allocation and cleans the active peer`, () => {
    const f = fixture('batches-cleanup-error', reuse);
    try {
      assert.throws(() => execFileSync('bash', f.args, {env: f.env, timeout: 15000, stdio: 'pipe'}), error => error.status === 1);
      assert.equal(f.calls().filter(row => row.command === 'docker' && row.args[0] === 'run').length, 4);
      assert.ok(existsSync(`${f.log}.started-2`));
      assert.equal(existsSync(`${f.log}.started-3`), false);
      assert.equal(f.containers().length, 1);
      assert.equal(f.volumes().length, 1);
      assert.ok(existsSync(f.foreignVolume));
    } finally { f.close(); }
  });

  test(`${mode}: volume creation, run and ownership failures retain foreign data and clean exact partial fixtures`, () => {
    for (const scenario of ['volume-list-error', 'foreign-volume', 'volume-create-error', 'run-error', 'volume-owner-mismatch']) {
      const f = fixture(scenario, reuse);
      try {
        assert.throws(() => execFileSync('bash', f.args, {env: f.env, timeout: 15000, stdio: 'pipe'}));
        const calls = f.calls();
        assert.equal(calls.filter(row => row.command === 'cargo' && row.args[1] === 'run').length, 0);
        assert.equal(f.containers().length, 0);
        assert.ok(existsSync(f.foreignVolume));
        const retained = ['foreign-volume', 'volume-owner-mismatch'].includes(scenario);
        assert.equal(f.volumes().length, retained ? (scenario === 'volume-owner-mismatch' ? 1 : 4) : 0);
        const removals = calls.filter(row => row.args[0] === 'volume' && row.args[1] === 'rm');
        assert.equal(removals.length, ['volume-create-error', 'run-error'].includes(scenario) ? 4 : 0);
        assert.ok(removals.every(row => /^cma-mongo-ci-[0-9a-f]{24}-data$/.test(row.args[2])));
        if (scenario === 'foreign-volume' || scenario === 'volume-list-error') {
          assert.equal(calls.filter(row => row.args[0] === 'volume' && row.args[1] === 'create').length, 0);
          assert.equal(calls.filter(row => row.args[0] === 'ps').length, 0, 'refuse before cleanup scope');
        }
      } finally { f.close(); }
    }
  });

  test(`${mode}: invalid partition arguments fail before Docker or Cargo`, () => {
    const f = fixture('success', reuse);
    try {
      for (const args of [['count:0/1'], ['count:3/2'], ['count:1/0'], ['count:01/2'], ['count:1/1000000'], ['count:1/2', 'extra']]) {
        assert.throws(() => execFileSync('bash', [...f.args, ...args], { env: f.env, stdio: 'pipe' }));
      }
      assert.equal(existsSync(f.log), false);
    } finally { f.close(); }
  });

  for (const scenario of ['cancel', 'cancel-scale', 'cancel-owner']) for (const signal of ['SIGTERM', 'SIGINT']) test(`${mode}: ${signal} stops ${scenario} process groups and removes their exact disk volumes`, async () => {
    const f = fixture(scenario, reuse);
    const active = scenario === 'cancel-scale' ? [0] : scenario === 'cancel-owner' ? [-1] : [1, 2];
    const child = spawn('bash', f.args, { env: f.env, stdio: 'ignore' });
    const exited = new Promise(resolve => child.once('exit', (code, signal) => resolve({code,signal})));
    try {
      const deadline = Date.now() + 10000;
      while (!active.every(index => existsSync(`${f.log}.started-${index}`))) {
        assert.ok(Date.now() < deadline, 'selected child Cargo processes must start');
        await new Promise(resolve => setTimeout(resolve, 10));
      }
      assert.equal(f.volumes().length, active.length);
      child.kill(signal);
      const result = await Promise.race([
        exited,
        new Promise((_, reject) => { const timer = setTimeout(() => reject(new Error('coordinator did not exit')), 15000); timer.unref(); }),
      ]);
      assert.deepEqual(result, {code: signal === 'SIGTERM' ? 143 : 130, signal: null});
      assert.equal(f.containers().length, 0);
      assert.equal(f.volumes().length, 0);
      assert.ok(existsSync(f.foreignVolume));
    } finally {
      if (child.exitCode === null) { child.kill('SIGTERM'); await exited; }
      f.close();
    }
  });


  test(`${mode}: larger suites use bounded fresh fixtures, with at most two running and all partitions attempted after failure`, () => {
    for (const scenario of ['batches', 'batches-error']) {
      const f = fixture(scenario, reuse);
      try {
        const run = () => execFileSync('bash', f.args, {env:f.env, timeout:30000, stdio:'pipe'});
        if (scenario === 'batches') run(); else assert.throws(run);
        const calls = f.calls();
        assert.equal(calls.filter(row => row.command === 'cargo' && row.args[1] === 'list').length, 1);
        const cargo = calls.filter(row => row.command === 'cargo' && row.args[1] === 'run');
        assert.ok(cargo.slice(0, 2).every(row => !row.args.includes('--partition')));
        assert.deepEqual(cargo.slice(2).map(row => row.args[row.args.indexOf('--partition')+1]).sort(),
          [1,2,3,4,5].map(index => `count:${index}/5`));
        assert.equal(new Set(cargo.map(row => row.uri)).size, 7);
        let active = 0;
        let maximum = 0;
        for (const row of calls) {
          if (row.command === 'docker' && row.args[0] === 'run') maximum = Math.max(maximum, ++active);
          if (row.command === 'docker' && row.args[0] === 'volume' && row.args[1] === 'rm') --active;
          assert.ok(active >= 0 && active <= 2);
        }
        assert.equal(maximum, 2);
        assert.equal(active, 0);
        assert.equal(f.containers().length, 0);
        assert.equal(f.volumes().length, 0);
        assert.ok(existsSync(f.foreignVolume));
        for (const index of [0,-1,1,2,3,4,5]) assert.ok(existsSync(`${f.log}.finished-${index}`));
      } finally { f.close(); }
    }
  });

  test(`${mode}: unavailable, malformed, empty or non-ignored selection fails before allocating any fixture`, () => {
    for (const scenario of ['list-error','list-malformed','list-empty','list-not-ignored','list-no-scale','list-scale-not-ignored','list-duplicate-scale','list-no-owner','list-owner-not-ignored','list-duplicate-owner','list-no-ordinary']) {
      const f = fixture(scenario, reuse);
      try {
        assert.throws(() => execFileSync('bash', f.args, {env:f.env, timeout:5000, stdio:'pipe'}));
        assert.deepEqual(f.calls().map(row => [row.command,row.args[1]]), [['cargo','list']]);
        assert.equal(f.containers().length, 0);
        assert.equal(f.volumes().length, 0);
        assert.ok(existsSync(f.foreignVolume));
      } finally { f.close(); }
    }
  });


  test(`${mode}: zero-test runs fail despite an environment that allows empty selections`, () => {
    for (const scenario of ['scale-empty', 'owner-empty', 'ordinary-empty']) {
      const f = fixture(scenario, reuse);
      try {
        const env = {...f.env, NEXTEST_NO_TESTS: 'pass'};
        assert.throws(() => execFileSync('bash', f.args, {env, timeout:15000, stdio:'pipe'}),
          error => error.status === 1, 'an empty invocation must fail the complete gate');
        const cargo = f.calls().filter(row => row.command === 'cargo' && row.args[1] === 'run');
        assert.equal(cargo.length, 4, 'the remaining contracts still run after an empty invocation');
        for (const row of cargo) {
          assert.equal(row.args.filter(value => value === '--no-tests').length, 1);
          assert.equal(row.args[row.args.indexOf('--no-tests')+1], 'fail');
        }
        const empty = scenario === 'scale-empty' ? 0 : scenario === 'owner-empty' ? -1 : 1;
        assert.equal(readFileSync(`${f.log}.empty-${empty}`, 'utf8'), 'fail');
        for (const index of [0,-1,1,2].filter(index => index !== empty)) {
          assert.ok(existsSync(`${f.log}.finished-${index}`));
        }
        assert.equal(f.containers().length, 0);
        assert.equal(f.volumes().length, 0);
        assert.ok(existsSync(f.foreignVolume));
      } finally { f.close(); }
    }
  });
}

test('selection proof parses both old and isolated owner predicates', () => {
  const source = readFileSync(new URL('mongo-contract-tests.sh', import.meta.url), 'utf8');
  const result = execFileSync('python3', ['-c', `
import json, runpy, sys
parse = runpy.run_path(sys.argv[1])["mongo_filters"]
source = sys.argv[2]
before = "\\n".join(line for line in source.splitlines()
    if not line.startswith("readonly owner_allocation_test=")
    and not line.strip().startswith("owner-allocation)"))
before = before.replace('not (test(=$scale_test) or test(=$owner_allocation_test))', 'not test(=$scale_test)')
print(json.dumps({"before": parse(before), "after": parse(source)}))
`, new URL('backend-selection-proof.py', import.meta.url).pathname, source], {
    env: {...process.env, PYTHONDONTWRITEBYTECODE: '1'}, encoding: 'utf8',
  });
  const {before, after} = JSON.parse(result);
  assert.equal(before['owner-allocation'], undefined);
  assert.ok(before.ordinary.startsWith(`not test(=${scaleTest}) & (`));
  assert.ok(after.ordinary.startsWith(`${ordinaryPredicate} & (`));
  assert.ok(after['owner-allocation'].startsWith(`package(=cma) and test(=${ownerTest}) & (`));
  for (const mode of ['all', 'scale', 'cpu-profile']) assert.equal(before[mode], after[mode]);
});
