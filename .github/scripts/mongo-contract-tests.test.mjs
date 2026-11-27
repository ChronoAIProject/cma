import assert from 'node:assert/strict';
import test from 'node:test';
import { execFileSync, spawn } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { requiredPrefixes } from './mongo-contract-test-prefixes.mjs';
import './mongo-contract-completeness.test.mjs';
import './mongo-contract-registry.test.mjs';
import './mongo-contract-space-path.test.mjs';
import { registerSelectionTests } from './mongo-contract-selection-tests.mjs';
import { registerShardTests } from './mongo-contract-shards.test.mjs';
import { registerRegistryRunnerTests } from './mongo-contract-registry-runner-tests.mjs';

import { scaleTest, ownerTest, auditTest, diagnosticsTest, scanTest, bsonCeilingTest, firstResponseTest, historyTest, rankedProjectionTest, isolatedTests, observationTest, runnerSetupTimeoutMs, fixturePredicate, qualificationPredicate, ordinaryPredicate, fixture } from './mongo-contract-test-fixture.mjs';

registerRegistryRunnerTests({fixture, requiredPrefixes});

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
        assert.ok(f.calls().every(row => row.authenticatedUri === undefined), 'ordinary fixtures discard authenticated caller authority');
        const build = reuse ? ['--binaries-metadata', join(f.metadata, 'binaries-metadata.json'), '--cargo-metadata', join(f.metadata, 'cargo-metadata.json')] : ['--workspace', '--locked'];
        const selection = `package(=cma) and test(::cpu_profile::) & ${fixturePredicate} & (${requiredPrefixes.map(prefix => `test(${prefix})`).join(' | ')})`;
        assert.deepEqual(cargo[0].args, ['nextest', 'run', ...build, '--no-tests', 'fail', '--test-threads', '1', '--no-fail-fast', '--success-output', 'final', '--run-ignored', 'only', '-E', selection]);
        assert.match(cargo[0].uri, /^mongodb:\/\/127\.0\.0\.1:[0-9]+\/\?directConnection=true&replicaSet=cma_ci$/);
        assert.equal(f.calls().filter(row => row.command === 'docker' && row.args[0] === 'run').length, 1);
        assert.equal(f.containers().length, 0);
        assert.equal(f.volumes().length, 0);
        assert.ok(existsSync(f.foreignVolume));
      } finally { f.close(); }
    }
  });

  test(`${mode}: the thirty-minute observation qualification runs only through its own fixture`, () => {
    for (const scenario of ['success', 'observation-error']) {
      const f = fixture(scenario, reuse);
      try {
        const run = () => execFileSync('bash', [...f.args, '--observation-budget'], {env: f.env, timeout: 15000, stdio: 'pipe'});
        if (scenario === 'success') run(); else assert.throws(run, error => error.status === 7);
        const cargo = f.calls().filter(row => row.command === 'cargo');
        assert.equal(cargo.length, 1, 'no listing or partition planning precedes the qualification');
        const build = reuse ? ['--binaries-metadata', join(f.metadata, 'binaries-metadata.json'), '--cargo-metadata', join(f.metadata, 'cargo-metadata.json')] : ['--workspace', '--locked'];
        const selection = `package(=cma) and test(=${observationTest}) & ${fixturePredicate} & (${requiredPrefixes.map(prefix => `test(${prefix})`).join(' | ')})`;
        assert.deepEqual(cargo[0].args, ['nextest', 'run', ...build, '--no-tests', 'fail', '--test-threads', '1', '--no-fail-fast', '--success-output', 'final', '--run-ignored', 'only', '-E', selection]);
        assert.match(cargo[0].uri, /^mongodb:\/\/127\.0\.0\.1:[0-9]+\/\?directConnection=true&replicaSet=cma_ci$/);
        assert.equal(f.calls().filter(row => row.command === 'docker' && row.args[0] === 'run').length, 1);
        assert.equal(f.containers().length, 0);
        assert.equal(f.volumes().length, 0);
        assert.ok(existsSync(f.foreignVolume));
      } finally { f.close(); }
    }
  });

  test(`${mode}: isolated contracts run alone before concurrent Mongo partitions, preserves failures and removes only owned fixtures`, () => {
    for (const scenario of ['entropy-error', 'entropy-malformed', 'success', 'scale-error', 'owner-error', 'audit-error', 'diagnostics-error', 'scan-error', 'bson-error', 'first-response-error', 'history-error', 'ranked-projection-error', 'test-error']) {
      const f = fixture(scenario, reuse);
      try {
        const run = () => execFileSync('bash', f.args, { env: f.env, timeout: runnerSetupTimeoutMs, stdio: 'pipe' });
        let stderr = '';
        if (scenario === 'success') run(); else assert.throws(run, error => { stderr = String(error.stderr); return true; });
        if (scenario.startsWith('entropy-')) { assert.deepEqual(f.calls().map(row => [row.command,row.args[1]]), [['cargo','list']]); continue; }
        assert.equal(stderr.includes('per-test budget of nextest profile mongo-ordinary'), scenario === 'test-error',
          'only a failed ordinary partition names the budget remedy');
        if (scenario === 'test-error') assert.match(stderr, /Mongo partition count:1\/2 failed \(exit 7\)[^]*dedicated mode[^]*mongo-contract-tests\.sh/);
        const calls = f.calls();
        assert.ok(calls.every(row => row.authenticatedUri === undefined), 'listing and execution discard authenticated caller authority');
        const cargo = calls.filter(row => row.command === 'cargo' && row.args[1] === 'run');
        for (const row of calls.filter(row => row.command === 'cargo')) {
          const action = row.args[1];
          const isolatedTest = isolatedTests[cargo.indexOf(row)];
          const predicate = action === 'list' ? qualificationPredicate
            : isolatedTest ? `package(=cma) and test(=${isolatedTest})` : ordinaryPredicate;
          const selection = `${predicate} & ${fixturePredicate} & (${requiredPrefixes.map(prefix => `test(${prefix})`).join(' | ')})`;
          const build = reuse ? ['--binaries-metadata', join(f.metadata, 'binaries-metadata.json'), '--cargo-metadata', join(f.metadata, 'cargo-metadata.json')] : ['--workspace', '--locked'];
          const operation = action === 'list' ? ['--message-format', 'json'] : ['--no-tests', 'fail', '--test-threads', '1', '--no-fail-fast', ...(cargo.indexOf(row) < isolatedTests.length ? ['--success-output', 'final', ...(isolatedTest === rankedProjectionTest ? ['--profile', 'mongo-ordinary'] : [])] : ['--partition', row.args[row.args.indexOf('--partition')+1], '--profile', 'mongo-ordinary'])];
          assert.deepEqual(row.args, ['nextest', action, ...build, ...operation, '--run-ignored', 'only', '-E', selection]);
          if (action === 'list') assert.equal(row.uri, undefined, 'listing must discard caller Mongo URI too');
        }

        assert.equal(cargo.length, isolatedTests.length + 2);
        assert.ok(cargo.slice(0, isolatedTests.length).every(row => !row.args.includes('--partition')));
        assert.deepEqual(cargo.slice(isolatedTests.length).map(row => row.args[row.args.indexOf('--partition') + 1]).sort(), ['count:1/2', 'count:2/2']);
        assert.equal(new Set(cargo.map(row => row.uri)).size, isolatedTests.length + 2);
        for (const row of cargo) {
          assert.match(row.uri, /^mongodb:\/\/127\.0\.0\.1:[0-9]+\/\?directConnection=true&replicaSet=cma_ci$/);
          assert.ok(row.args.includes('--run-ignored') && row.args.includes('only'));
          assert.ok(row.args.includes('--no-fail-fast'));
          assert.equal(row.args[row.args.indexOf('--test-threads') + 1], '1');
        }
        for (const index of [0, -1, -2, -3, -4, -5, -6, -7, -8, 1, 2]) assert.ok(existsSync(`${f.log}.finished-${index}`), 'both partitions must finish, including after one fails');
        const scale = JSON.parse(readFileSync(`${f.log}.selected-0`));
        const owner = JSON.parse(readFileSync(`${f.log}.selected--1`));
        const audit = JSON.parse(readFileSync(`${f.log}.selected--2`));
        const diagnostics = JSON.parse(readFileSync(`${f.log}.selected--3`));
        assert.deepEqual(diagnostics, [diagnosticsTest]);
        const scan = JSON.parse(readFileSync(`${f.log}.selected--4`));
        assert.deepEqual(scan, [scanTest]);
        const bson = JSON.parse(readFileSync(`${f.log}.selected--5`));
        assert.deepEqual(bson, [bsonCeilingTest]);
        const firstResponse = JSON.parse(readFileSync(`${f.log}.selected--6`));
        assert.deepEqual(firstResponse, [firstResponseTest]);
        const history = JSON.parse(readFileSync(`${f.log}.selected--7`));
        assert.deepEqual(history, [historyTest]);
        const rankedProjection = JSON.parse(readFileSync(`${f.log}.selected--8`));
        assert.deepEqual(rankedProjection, [rankedProjectionTest]);
        const ordinary = [1, 2].flatMap(index => JSON.parse(readFileSync(`${f.log}.selected-${index}`)));
        assert.deepEqual(scale, [scaleTest]);
        assert.deepEqual(owner, [ownerTest]);
        assert.deepEqual(audit, [auditTest]);
        assert.equal(ordinary.length, 80);
        assert.equal(new Set([...scale, ...owner, ...audit, ...diagnostics, ...scan, ...bson, ...firstResponse, ...history, ...rankedProjection, ...ordinary]).size, isolatedTests.length + 80, 'all isolated phases plus ordinary partitions select every mock test exactly once');
        const created = calls.filter(row => row.args[0] === 'run').map(row => row.args[row.args.indexOf('--name') + 1]);
        assert.equal(new Set(created).size, isolatedTests.length + 2);
        for (let index = 0; index < isolatedTests.length; index++) {
          const cleanup = calls.findIndex(row => row.args[0] === 'volume' && row.args[1] === 'rm' && row.args[2] === `${created[index]}-data`);
          const nextStart = calls.findIndex(row => row.args[0] === 'run' && row.args.includes(created[index+1]));
          assert.ok(cleanup >= 0 && nextStart > cleanup, 'each exclusive fixture is removed before the next phase starts');
        }
        assert.ok(created.every(name => /^cma-mongo-ci-[0-9a-f]{24}$/.test(name)));
        const volumeCreates = calls.filter(row => row.args[0] === 'volume' && row.args[1] === 'create');
        assert.equal(volumeCreates.length, isolatedTests.length + 2);
        for (const name of created) {
          assert.deepEqual(volumeCreates.find(row => row.args.at(-1) === `${name}-data`).args,
            ['volume', 'create', '--label', `dev.chrono.cma.mongo-contract=${name}`, `${name}-data`]);
          const run = calls.find(row => row.args[0] === 'run' && row.args.includes(name));
          assert.equal(run.args[run.args.indexOf('--mount') + 1], `type=volume,source=${name}-data,target=/data/db`);
          assert.equal(run.args[run.args.indexOf('--tmpfs') + 1], '/data/configdb:rw,size=16777216');
          assert.equal(run.args.filter(value => value === '--tmpfs').length, 1);
          assert.equal(run.args[run.args.indexOf('--memory') + 1], '2g');
          assert.equal(run.args[run.args.indexOf('--memory-swap') + 1], '2g');
          assert.equal(run.args[run.args.indexOf('--cpus') + 1], '2');
          assert.equal(run.args[run.args.indexOf('--ulimit') + 1], 'nofile=64000:64000');
          const expectedCache = name === created[isolatedTests.indexOf(bsonCeilingTest)] ? '0.50' : '0.25';
          assert.equal(run.args[run.args.indexOf('--wiredTigerCacheSizeGB') + 1], expectedCache);
          const removedVolume = calls.find(row => row.args[0] === 'volume' && row.args[1] === 'rm' && row.args[2] === `${name}-data`);
          assert.deepEqual(removedVolume.args, ['volume', 'rm', `${name}-data`]);
          assert.ok(calls.slice(0, calls.indexOf(removedVolume)).some(row => row.args[0] === 'volume' && row.args[1] === 'inspect' && row.args.at(-1) === `${name}-data`));
        }
        assert.ok(calls.filter(row => row.args[0] === 'ps').every(row => created.some(name => row.args.includes(`label=dev.chrono.cma.mongo-contract=${name}`))));
        const removed = calls.filter(row => row.args[0] === 'rm');
        assert.equal(removed.length, isolatedTests.length + 2);
        assert.ok(removed.every(row => row.args.length === 3 && row.args[1] === '--force' && /^[0-9a-f]{12}$/.test(row.args[2])));
        assert.equal(f.containers().length, 0);
        assert.equal(f.volumes().length, 0);
        assert.ok(existsSync(f.foreignVolume));
      } finally { f.close(); }
    }
  });

  test(`${mode}: uncertain exclusive cleanup stops before another fixture can overlap`, () => {
    for (const scenario of ['cleanup-error', 'volume-cleanup-error', 'owner-cleanup-error', 'owner-volume-cleanup-error', 'audit-cleanup-error', 'audit-volume-cleanup-error', 'diagnostics-cleanup-error', 'scan-cleanup-error', 'diagnostics-volume-cleanup-error', 'scan-volume-cleanup-error', 'history-cleanup-error', 'history-volume-cleanup-error']) {
      const f = fixture(scenario, reuse);
      try {
        assert.throws(() => execFileSync('bash', f.args, {env: f.env, timeout: runnerSetupTimeoutMs, stdio: 'pipe'}), error => error.status === 1);
        const created = f.calls().filter(row => row.command === 'docker' && row.args[0] === 'run');
        assert.equal(created.length, scenario.startsWith('history-') ? 8 : scenario.startsWith('scan-') ? 5 : scenario.startsWith('diagnostics-') ? 4 : scenario.startsWith('audit-') ? 3 : scenario.startsWith('owner-') ? 2 : 1);
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
      assert.throws(() => execFileSync('bash', f.args, {env: f.env, timeout: runnerSetupTimeoutMs, stdio: 'pipe'}), error => error.status === 1);
      assert.equal(f.calls().filter(row => row.command === 'docker' && row.args[0] === 'run').length, isolatedTests.length + 2);
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
        assert.throws(() => execFileSync('bash', f.args, {env: f.env, timeout: runnerSetupTimeoutMs, stdio: 'pipe'}));
        const calls = f.calls();
        assert.equal(calls.filter(row => row.command === 'cargo' && row.args[1] === 'run').length, 0);
        assert.equal(f.containers().length, 0);
        assert.ok(existsSync(f.foreignVolume));
        const retained = ['foreign-volume', 'volume-owner-mismatch'].includes(scenario);
        assert.equal(f.volumes().length, retained ? (scenario === 'volume-owner-mismatch' ? 1 : isolatedTests.length + 2) : 0);
        const removals = calls.filter(row => row.args[0] === 'volume' && row.args[1] === 'rm');
        assert.equal(removals.length, ['volume-create-error', 'run-error'].includes(scenario) ? isolatedTests.length + 2 : 0);
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

  for (const scenario of ['cancel', 'cancel-scale', 'cancel-owner', 'cancel-audit', 'cancel-diagnostics', 'cancel-scan', 'cancel-history']) for (const signal of ['SIGTERM', 'SIGINT']) test(`${mode}: ${signal} stops ${scenario} process groups and removes their exact disk volumes`, async () => {
    const f = fixture(scenario, reuse);
    const active = scenario === 'cancel-scale' ? [0] : scenario === 'cancel-owner' ? [-1] : scenario === 'cancel-audit' ? [-2] : scenario === 'cancel-diagnostics' ? [-3] : scenario === 'cancel-scan' ? [-4] : scenario === 'cancel-history' ? [-7] : [1, 2];
    const child = spawn('bash', f.args, { env: f.env, stdio: 'ignore' });
    const exited = new Promise(resolve => child.once('exit', (code, signal) => resolve({code,signal})));
    try {
      const deadline = Date.now() + runnerSetupTimeoutMs;
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
        assert.ok(cargo.slice(0, isolatedTests.length).every(row => !row.args.includes('--partition')));
        assert.deepEqual(cargo.slice(isolatedTests.length).map(row => row.args[row.args.indexOf('--partition')+1]).sort(),
          [1,2,3,4,5].map(index => `count:${index}/5`));
        assert.equal(new Set(cargo.map(row => row.uri)).size, isolatedTests.length + 5);
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
        for (const index of [0,-1,-2,-3,-4,-5,-6,-7,-8,1,2,3,4,5]) assert.ok(existsSync(`${f.log}.finished-${index}`));
      } finally { f.close(); }
    }
  });

  test(`${mode}: unavailable, malformed, empty or non-ignored selection fails before allocating any fixture`, () => {
    for (const scenario of ['list-omitted-module','list-selected-helper','list-stale-exception','list-error','list-malformed','list-empty','list-not-ignored','list-no-scale','list-scale-not-ignored','list-duplicate-scale','list-no-owner','list-owner-not-ignored','list-duplicate-owner','list-no-audit','list-audit-not-ignored','list-duplicate-audit','list-no-diagnostics','list-diagnostics-not-ignored','list-duplicate-diagnostics','list-no-scan','list-scan-not-ignored','list-duplicate-scan','list-no-bson','list-bson-not-ignored','list-duplicate-bson','list-no-first-response','list-first-response-not-ignored','list-duplicate-first-response','list-no-history','list-history-not-ignored','list-duplicate-history','list-no-ranked-projection','list-ranked-projection-not-ignored','list-duplicate-ranked-projection','list-no-ordinary']) {
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
    for (const scenario of ['scale-empty', 'owner-empty', 'audit-empty', 'diagnostics-empty', 'scan-empty', 'bson-empty', 'first-response-empty', 'history-empty', 'ranked-projection-empty', 'ordinary-empty']) {
      const f = fixture(scenario, reuse);
      try {
        const env = {...f.env, NEXTEST_NO_TESTS: 'pass'};
        assert.throws(() => execFileSync('bash', f.args, {env, timeout:runnerSetupTimeoutMs, stdio:'pipe'}),
          error => error.status === 1, 'an empty invocation must fail the complete gate');
        const cargo = f.calls().filter(row => row.command === 'cargo' && row.args[1] === 'run');
        assert.equal(cargo.length, isolatedTests.length + 2, 'the remaining contracts still run after an empty invocation');
        for (const row of cargo) {
          assert.equal(row.args.filter(value => value === '--no-tests').length, 1);
          assert.equal(row.args[row.args.indexOf('--no-tests')+1], 'fail');
        }
        const empty = scenario === 'scale-empty' ? 0 : scenario === 'owner-empty' ? -1 : scenario === 'audit-empty' ? -2 : scenario === 'diagnostics-empty' ? -3 : scenario === 'scan-empty' ? -4 : scenario === 'bson-empty' ? -5 : scenario === 'first-response-empty' ? -6 : scenario === 'history-empty' ? -7 : scenario === 'ranked-projection-empty' ? -8 : 1;
        assert.equal(readFileSync(`${f.log}.empty-${empty}`, 'utf8'), 'fail');
        for (const index of [0,-1,-2,-3,-4,-5,-6,-7,-8,1,2].filter(index => index !== empty)) {
          assert.ok(existsSync(`${f.log}.finished-${index}`));
        }
        assert.equal(f.containers().length, 0);
        assert.equal(f.volumes().length, 0);
        assert.ok(existsSync(f.foreignVolume));
      } finally { f.close(); }
    }
  });
}

// Owner-approved merge-queue retry (.github/scripts/mongo_flaky_retry.py):
// only an ordinary partition retries, only once, and only when the workflow's
// merge-group policy step passed 1. Every other run, and every dedicated
// mode, keeps zero retries whatever the nextest environment says.
test('archive: only ordinary partitions retry once, and only when the merge-queue policy enables it', () => {
  // '' is what a skipped policy step (every PR and push) passes; unset is
  // every other suite here, which pins the complete argument lists.
  for (const setting of ['', '0', '1']) {
    const f = fixture('success', true);
    try {
      const reports = join(f.directory, 'flaky-reports');
      const env = {...f.env, NEXTEST_RETRIES: '5', NEXTEST_FLAKY_RESULT: 'fail'};
      delete env.CMA_MONGO_FLAKY_RETRIES;
      delete env.CMA_MONGO_FLAKY_REPORT_DIR;
      env.CMA_MONGO_FLAKY_RETRIES = setting;
      if (setting === '1') { mkdirSync(reports); env.CMA_MONGO_FLAKY_REPORT_DIR = reports; }
      execFileSync('bash', f.args, {env, timeout: runnerSetupTimeoutMs, stdio: 'pipe'});
      const cargo = f.calls().filter(row => row.command === 'cargo' && row.args[1] === 'run');
      assert.equal(cargo.length, isolatedTests.length + 2);
      for (const row of cargo) {
        assert.equal(row.retriesEnv, undefined, `${setting}: NEXTEST_RETRIES never reaches a run`);
        assert.equal(row.flakyResultEnv, undefined, `${setting}: NEXTEST_FLAKY_RESULT never reaches a run`);
        const partition = row.args.includes('--partition') ? row.args[row.args.indexOf('--partition') + 1] : null;
        const selection = row.args[row.args.indexOf('-E') + 1];
        const rankedProjection = selection.startsWith(`package(=cma) and test(=${rankedProjectionTest})`);
        const expected = partition || rankedProjection ? ['--profile', 'mongo-ordinary'] : [];
        if (setting === '1' && partition) {
          const stem = partition.replace(/[:/]/g, '-');
          expected.push('--retries', '1', '--flaky-result', 'pass', '--tool-config-file', `cma-flaky-retry:${reports}/nextest-${stem}.toml`);
          assert.equal(readFileSync(join(reports, `nextest-${stem}.toml`), 'utf8'),
            `[profile.mongo-ordinary.junit]\npath = "${reports}/junit-${stem}.xml"\n`, 'the tool config only sets the JUnit path');
        }
        const firstOption = row.args.indexOf(partition ? '--partition' : '--success-output') + 2;
        const options = row.args.slice(firstOption, row.args.indexOf('--run-ignored'));
        assert.deepEqual(options, expected, `${setting}: ${partition ?? 'isolated'} profile and retry options`);
      }
      assert.equal(cargo.filter(row => row.args.includes('--retries')).length, setting === '1' ? 2 : 0);
    } finally { f.close(); }
  }
});

test('archive: an invalid merge-queue retry setting fails before any listing or fixture', () => {
  for (const [setting, directory] of [['2', null], ['true', null], ['1', null], ['1', 'relative/flaky'], ['1', '/nonexistent-cma-flaky-dir'], ['1', 'with space']]) {
    const f = fixture('success', true);
    try {
      const env = {...f.env, CMA_MONGO_FLAKY_RETRIES: setting};
      delete env.CMA_MONGO_FLAKY_REPORT_DIR;
      if (directory === 'with space') { mkdirSync(join(f.directory, directory)); env.CMA_MONGO_FLAKY_REPORT_DIR = join(f.directory, directory); }
      else if (directory !== null) env.CMA_MONGO_FLAKY_REPORT_DIR = directory;
      assert.throws(() => execFileSync('bash', f.args, {env, timeout: 15000, stdio: 'pipe'}), error => error.status === 2,
        `${setting} ${directory} must be refused`);
      assert.equal(existsSync(f.log), false, 'nothing ran');
    } finally { f.close(); }
  }
});

test('no nextest profile configures retries, so only the merge-queue flag can', () => {
  const config = readFileSync(new URL('../../api/.config/nextest.toml', import.meta.url), 'utf8');
  assert.equal(config.split('\n').filter(line => /^\s*(retries|flaky-result)\s*=/.test(line)).length, 0);
});

registerSelectionTests({scaleTest, ownerTest, auditTest, diagnosticsTest, scanTest, bsonCeilingTest, firstResponseTest, historyTest, rankedProjectionTest, observationTest, fixturePredicate, ordinaryPredicate, qualificationPredicate});
registerShardTests({fixture, scaleTest, ownerTest, auditTest, diagnosticsTest, scanTest, bsonCeilingTest, firstResponseTest, historyTest, rankedProjectionTest});
