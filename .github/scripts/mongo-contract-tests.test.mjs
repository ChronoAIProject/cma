import assert from 'node:assert/strict';
import test from 'node:test';
import { execFileSync, spawn } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const requiredPrefixes = [
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
  'store::agent_access_tools::tests::',
  'store::agent_access_requests::',
  'agent_access_worker::',
  'owner_service_access::',
  'store::sandbox_mongo_integration_tests::warm_hierarchy_tests::',
  'store::agent_admission::mongo_integration_tests::',
  'store::schema_mongo_integration_tests::',
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
  'store::sandbox_mongo_integration_tests::sandbox_lifecycle_tests::lifecycle_phase_reconciles_an_ambiguous_commit_without_duplicate_logs',
  'sandbox::agent_worker::prepared_open_ready::',
];

function fixture(scenario) {
  const directory = mkdtempSync(join(tmpdir(), 'cma-mongo-runner-test-'));
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
  if (args[1] === 'list') {
    if (scenario === 'list-error') process.exit(7);
    if (scenario === 'list-malformed') { console.log('invalid-json'); process.exit(0); }
    const count = scenario === 'list-empty' ? 0 : scenario.startsWith('batches') ? 161 : 80;
    const testcases = Object.fromEntries(Array.from({length:count}, (_, index) => [
      'contract_'+index, {ignored:scenario !== 'list-not-ignored', 'filter-match':{status:'matches'}},
    ]));
    testcases.filtered = {ignored:true, 'filter-match':{status:'mismatch'}};
    console.log(JSON.stringify({'rust-suites':{fixture:{testcases}}}));
    process.exit(0);
  }
  const partition = args[args.indexOf('--partition') + 1];
  const match = /^count:([1-9][0-9]*)\\/([1-9][0-9]*)$/.exec(partition);
  if (!match) process.exit(8);
  const index = Number(match[1]);
  fs.writeFileSync(log+'.started-'+index, 'started');
  if (scenario === 'cancel') { setInterval(() => {}, 1000); }
  else {
    const deadline = Date.now()+5000;
    while (index <= 2 && !fs.existsSync(log+'.started-'+(3-index))) {
      if (Date.now() >= deadline) process.exit(9);
      Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 10);
    }
    fs.writeFileSync(log+'.finished-'+index, 'finished');
    process.exit(['test-error','batches-error'].includes(scenario) && index === 1 ? 7 : 0);
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
      if (scenario === 'volume-cleanup-error' || fs.existsSync(file(owner))) process.exit(21);
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
    if (scenario === 'cleanup-error') process.exit(1);
    const files = fs.readdirSync(path.dirname(log)).filter(name => name.startsWith('calls.ndjson.container-'));
    // Match immutable mock identity without reading another concurrently removed file.
    const selected = files.find(name => require('node:crypto').createHash('sha256')
      .update(name.slice('calls.ndjson.container-'.length)).digest('hex').startsWith(args[2]));
    if (!selected) process.exit(10);
    fs.unlinkSync(path.join(path.dirname(log),selected));
  } else process.exit(11);
}
`;
  for (const name of ['openssl', 'docker', 'cargo']) writeFileSync(join(directory, name), cli, { mode: 0o755 });
  const env = {
    ...process.env, PATH: `${directory}:${process.env.PATH}`, CMA_RUNNER_SCENARIO: scenario, CMA_RUNNER_LOG: log,
    CHRONO_TEST_MONGODB_URI: 'must-not-use-caller-uri', CHRONO_TEST_MONGODB_FAILPOINT_URI: 'must-not-use-caller-uri',
  };
  return {
    directory, log, env, foreignVolume,
    args: [new URL('./mongo-contract-tests.sh', import.meta.url).pathname],
    calls: () => readFileSync(log, 'utf8').trim().split('\n').map(line => JSON.parse(line)),
    containers: () => readdirSync(directory).filter(name => name.startsWith('calls.ndjson.container-')),
    volumes: () => readdirSync(directory).filter(name => /^calls\.ndjson\.volume-cma-mongo-ci-/.test(name)),
    close: () => rmSync(directory, { recursive: true }),
  };
}

test('both Mongo partitions run concurrently and remove only their owned containers and disk volumes', () => {
  for (const scenario of ['entropy-error', 'entropy-malformed', 'success', 'test-error', 'cleanup-error', 'volume-cleanup-error']) {
    const f = fixture(scenario);
    try {
      const run = () => execFileSync('bash', f.args, { env: f.env, timeout: 15000, stdio: 'pipe' });
      if (scenario === 'success') run(); else assert.throws(run);
      if (scenario.startsWith('entropy-')) { assert.deepEqual(f.calls().map(row => [row.command,row.args[1]]), [['cargo','list']]); continue; }
      const calls = f.calls();
      const cargo = calls.filter(row => row.command === 'cargo' && row.args[1] === 'run');
      assert.equal(cargo.length, 2);
      assert.deepEqual(cargo.map(row => row.args[row.args.indexOf('--partition') + 1]).sort(), ['count:1/2', 'count:2/2']);
      assert.equal(new Set(cargo.map(row => row.uri)).size, 2);
      for (const row of cargo) {
        assert.match(row.uri, /^mongodb:\/\/127\.0\.0\.1:[0-9]+\/\?directConnection=true&replicaSet=cma_ci$/);
        assert.ok(row.args.includes('--run-ignored') && row.args.includes('only'));
        assert.ok(row.args.includes('--no-fail-fast'));
        assert.equal(row.args[row.args.indexOf('--test-threads') + 1], '1');
        assert.equal(row.args[row.args.indexOf('-E') + 1], requiredPrefixes.map(prefix => `test(${prefix})`).join(' | '));
      }
      for (const index of [1, 2]) assert.ok(existsSync(`${f.log}.finished-${index}`), 'both partitions must finish, including after one fails');
      const created = calls.filter(row => row.args[0] === 'run').map(row => row.args[row.args.indexOf('--name') + 1]);
      assert.equal(new Set(created).size, 2);
      assert.ok(created.every(name => /^cma-mongo-ci-[0-9a-f]{24}$/.test(name)));
      const volumeCreates = calls.filter(row => row.args[0] === 'volume' && row.args[1] === 'create');
      assert.equal(volumeCreates.length, 2);
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
      assert.equal(removed.length, 2);
      assert.ok(removed.every(row => row.args.length === 3 && row.args[1] === '--force' && /^[0-9a-f]{12}$/.test(row.args[2])));
      assert.equal(f.containers().length, scenario === 'cleanup-error' ? 2 : 0);
      assert.equal(f.volumes().length, scenario.endsWith('cleanup-error') ? 2 : 0);
      assert.ok(existsSync(f.foreignVolume));
    } finally { f.close(); }
  }
});

test('volume creation, run and ownership failures retain foreign data and clean exact partial fixtures', () => {
  for (const scenario of ['volume-list-error', 'foreign-volume', 'volume-create-error', 'run-error', 'volume-owner-mismatch']) {
    const f = fixture(scenario);
    try {
      assert.throws(() => execFileSync('bash', f.args, {env: f.env, timeout: 15000, stdio: 'pipe'}));
      const calls = f.calls();
      assert.equal(calls.filter(row => row.command === 'cargo' && row.args[1] === 'run').length, 0);
      assert.equal(f.containers().length, 0);
      assert.ok(existsSync(f.foreignVolume));
      const retained = ['foreign-volume', 'volume-owner-mismatch'].includes(scenario);
      assert.equal(f.volumes().length, retained ? 2 : 0);
      const removals = calls.filter(row => row.args[0] === 'volume' && row.args[1] === 'rm');
      assert.equal(removals.length, ['volume-create-error', 'run-error'].includes(scenario) ? 2 : 0);
      assert.ok(removals.every(row => /^cma-mongo-ci-[0-9a-f]{24}-data$/.test(row.args[2])));
      if (scenario === 'foreign-volume' || scenario === 'volume-list-error') {
        assert.equal(calls.filter(row => row.args[0] === 'volume' && row.args[1] === 'create').length, 0);
        assert.equal(calls.filter(row => row.args[0] === 'ps').length, 0, 'refuse before cleanup scope');
      }
    } finally { f.close(); }
  }
});

test('invalid partition arguments fail before Docker or Cargo', () => {
  const f = fixture('success');
  try {
    for (const args of [['count:0/1'], ['count:3/2'], ['count:1/0'], ['count:01/2'], ['count:1/1000000'], ['count:1/2', 'extra']]) {
      assert.throws(() => execFileSync('bash', [...f.args, ...args], { env: f.env, stdio: 'pipe' }));
    }
    assert.equal(existsSync(f.log), false);
  } finally { f.close(); }
});

for (const signal of ['SIGTERM', 'SIGINT']) test(`${signal} stops both process groups and removes their exact disk volumes`, async () => {
  const f = fixture('cancel');
  const child = spawn('bash', f.args, { env: f.env, stdio: 'ignore' });
  const exited = new Promise(resolve => child.once('exit', (code, signal) => resolve({code,signal})));
  try {
    const deadline = Date.now() + 10000;
    while (![1, 2].every(index => existsSync(`${f.log}.started-${index}`))) {
      assert.ok(Date.now() < deadline, 'both child Cargo processes must start');
      await new Promise(resolve => setTimeout(resolve, 10));
    }
    assert.equal(f.volumes().length, 2);
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


test('larger suites use bounded fresh fixtures, with at most two running and all partitions attempted after failure', () => {
  for (const scenario of ['batches', 'batches-error']) {
    const f = fixture(scenario);
    try {
      const run = () => execFileSync('bash', f.args, {env:f.env, timeout:30000, stdio:'pipe'});
      if (scenario === 'batches') run(); else assert.throws(run);
      const calls = f.calls();
      assert.equal(calls.filter(row => row.command === 'cargo' && row.args[1] === 'list').length, 1);
      const cargo = calls.filter(row => row.command === 'cargo' && row.args[1] === 'run');
      assert.deepEqual(cargo.map(row => row.args[row.args.indexOf('--partition')+1]).sort(),
        [1,2,3,4,5].map(index => `count:${index}/5`));
      assert.equal(new Set(cargo.map(row => row.uri)).size, 5);
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
      for (const index of [1,2,3,4,5]) assert.ok(existsSync(`${f.log}.finished-${index}`));
    } finally { f.close(); }
  }
});

test('unavailable, malformed, empty or non-ignored selection fails before allocating any fixture', () => {
  for (const scenario of ['list-error','list-malformed','list-empty','list-not-ignored']) {
    const f = fixture(scenario);
    try {
      assert.throws(() => execFileSync('bash', f.args, {env:f.env, timeout:5000, stdio:'pipe'}));
      assert.deepEqual(f.calls().map(row => [row.command,row.args[1]]), [['cargo','list']]);
      assert.equal(f.containers().length, 0);
      assert.equal(f.volumes().length, 0);
      assert.ok(existsSync(f.foreignVolume));
    } finally { f.close(); }
  }
});
