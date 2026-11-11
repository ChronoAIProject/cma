import assert from 'node:assert/strict';
import test from 'node:test';
import { execFileSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

// Each row is one former invocation, including both include-ignored cases and
// nextest's existing exact owner-key contract. Match substrings explicitly.
const selections = [
  ['package(=cma-image-manager) and kind(test) and (binary(=mongo_isolation) or binary(=browser_session))', 'only', true],
  ['package(=cmaeg) and kind(lib) and test(~optional_forms_storage)', 'only', true],
  ['package(=cmaeg) and kind(lib) and test(~optional_media_continuation_storage)', 'only', true],
  ['package(=cmaeg) and test(=worker_credentials::mongo::integration_tests::worker_history_cross_client_visibility_append_only_grants_and_cutover_barrier)', 'only', false],
  ['package(=cmaeg) and kind(lib) and test(~whatsapp_retention_on_real_mongo)', 'only', true],
  ['package(=cmaeg) and kind(test) and binary(=mongo_isolation)', 'only', true],
  ['package(=cmaeg) and kind(test) and binary(=management) and test(~github_integrations::)', 'only', true],
  ['package(=cmaeg) and kind(test) and binary(=management) and test(~organization_channels::)', 'only', true],
  ['package(=cmaeg) and kind(test) and binary(=github_source)', 'only', true],
  ['package(=cmaeg) and kind(lib) and test(~tools::threads::ledger::mongo_tests::)', 'only', true],
  ['package(=cma) and test(~store::isolation::tests::real_)', 'only', false],
  ['package(=cmaeg) and kind(test) and binary(=management) and test(~binary::compiled_binary_)', 'all', true],
  ['package(=cmaeg) and kind(test) and binary(=relay_ingress) and test(~runner_binary::)', 'only', true],
  ['package(=cma) and test(~gateway::tests::)', 'only', false],
  ['package(=cma) and test(~sender_context_tests::)', 'only', true],
];

function fixture(reuse, scenario = 'success') {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), 'cma-image-manager-runner-')));
  const root = join(directory, 'repo');
  const scripts = join(root, '.github/scripts');
  const metadata = join(root, 'api/target/nextest');
  mkdirSync(scripts, {recursive: true});
  mkdirSync(metadata, {recursive: true});
  for (const name of ['image-manager-mongo-contract.sh', 'authenticated-mongo-readiness.js', 'backend-nextest.sh']) {
    copyFileSync(new URL(name, import.meta.url), join(scripts, name));
  }
  writeFileSync(join(metadata, 'cargo-metadata.json'), JSON.stringify({workspace_root: join(root, 'api'), target_directory: join(root, 'api/target')}));
  writeFileSync(join(metadata, 'binaries-metadata.json'), JSON.stringify({'rust-build-meta': {'target-directory': join(root, 'api/target')}}));
  const log = join(directory, 'calls.ndjson');
  const cli = `#!/usr/bin/env node
const fs = require('node:fs');
const path = require('node:path');
const args = process.argv.slice(2);
const command = path.basename(process.argv[1]);
const log = process.env.CMA_TEST_LOG;
const scenario = process.env.CMA_TEST_SCENARIO;
if (command === 'openssl') { console.log('ab'.repeat(Number(args[2]))); process.exit(0); }
fs.appendFileSync(log, JSON.stringify({command,args,uri:process.env.CMA_IM_TEST_MONGODB_URI,
  caller:process.env.CHRONO_TEST_MONGODB_URI,failpoint:process.env.CHRONO_TEST_MONGODB_FAILPOINT_URI})+'\\n');
if (command === 'cargo') {
  if (scenario === 'test-error') process.exit(8);
} else if (args[0] === 'run') {
  if (scenario === 'foreign-container') process.exit(9);
  console.log('fixture-container');
} else if (args[0] === 'port') console.log('127.0.0.1:32778');
else if (args[0] === 'inspect') console.log(scenario === 'foreign-container' ? 'foreign' : 'ab'.repeat(12));
else if (args[0] === 'rm' && scenario === 'cleanup-error') process.exit(10);
`;
  for (const command of ['cargo', 'docker', 'openssl']) writeFileSync(join(directory, command), cli, {mode: 0o755});
  const env = {...process.env, CI: reuse ? 'true' : 'false', PATH: `${directory}:${process.env.PATH}`,
    CMA_TEST_LOG: log, CMA_TEST_SCENARIO: scenario, CMA_IM_TEST_MONGODB_URI: 'caller-uri',
    CHRONO_TEST_MONGODB_URI: 'caller-uri', CHRONO_TEST_MONGODB_FAILPOINT_URI: 'caller-uri'};
  if (reuse) env.CMA_NEXTEST_REUSE = '1'; else delete env.CMA_NEXTEST_REUSE;
  return {
    metadata, env, log,
    run: () => execFileSync('bash', [join(scripts, 'image-manager-mongo-contract.sh')], {env, stdio: 'pipe', timeout: 15000}),
    calls: () => existsSync(log) ? readFileSync(log, 'utf8').trim().split('\n').map(row => JSON.parse(row)) : [],
    close: () => rmSync(directory, {recursive: true}),
  };
}

for (const reuse of [false, true]) {
  test(`${reuse ? 'archive' : 'source'}: every Image Manager selection is serial and uses its owned authenticated fixture`, () => {
    const f = fixture(reuse);
    try {
      f.run();
      const calls = f.calls();
      const cargo = calls.filter(row => row.command === 'cargo');
      assert.equal(selections.length, 15);
      assert.equal(cargo.length, selections.length, 'no cargo build/test or hidden recompilation in archive mode');
      for (const [index, row] of cargo.entries()) {
        const [selection, ignored, converted] = selections[index];
        const build = reuse ? ['--binaries-metadata', join(f.metadata, 'binaries-metadata.json'), '--cargo-metadata', join(f.metadata, 'cargo-metadata.json')] : ['--workspace', '--locked'];
        const args = ['nextest', 'run', ...build, '--no-tests', 'fail'];
        if (converted) args.push('--run-ignored', ignored, '--test-threads', '1', '--no-fail-fast', '-E', selection);
        else args.push('-E', selection, '--run-ignored', ignored, '--test-threads', '1');
        assert.deepEqual(row.args, args);
        assert.match(row.uri, /^mongodb:\/\/contract-admin:[a-f0-9]+@127\.0\.0\.1:32778\/admin\?directConnection=true$/);
      }
      for (const row of calls) {
        const sender = row.command === 'cargo' && row.args.includes('package(=cma) and test(~sender_context_tests::)');
        assert.equal(row.caller, sender ? row.uri : undefined, 'only sender contracts receive the owned replica-set authority');
        assert.equal(row.failpoint, undefined);
      }
      assert.ok(calls.filter(row => row.command === 'docker' && row.args[0] !== 'rm' && row.args[0] !== 'container' && row.args[0] !== 'inspect').every(row => row.uri === undefined));
      const created = calls.find(row => row.command === 'docker' && row.args[0] === 'run');
      const name = `cma-im-mongo-${'ab'.repeat(12)}`;
      assert.equal(created.args[created.args.indexOf('--name')+1], name);
      assert.equal(created.args[created.args.indexOf('--label')+1], `dev.chrono.cma.im-contract=${'ab'.repeat(12)}`);
      assert.ok(created.args.includes('mongo:8.0.13@sha256:cf340b1e5283843c63eb12999922f20c463ae31285f746d30f05dcc21cd1d47c'));
      assert.match(created.args.at(-1), /--setParameter enableTestCommands=1$/);
      assert.deepEqual(calls.at(-1).args, ['rm', '--force', '--volumes', name]);
      assert.deepEqual(calls.at(-2).args, ['inspect', '--format', '{{index .Config.Labels "dev.chrono.cma.im-contract"}}', name]);
    } finally { f.close(); }
  });
  for (const scenario of ['test-error', 'cleanup-error', 'foreign-container']) test(`${reuse ? 'archive' : 'source'}: ${scenario} fails without deleting foreign containers`, () => {
    const f = fixture(reuse, scenario);
    try {
      assert.throws(f.run);
      const removals = f.calls().filter(row => row.args[0] === 'rm');
      assert.equal(removals.length, scenario === 'foreign-container' ? 0 : 1);
      assert.equal(f.calls().filter(row => row.command === 'cargo').length, scenario === 'foreign-container' ? 0 : scenario === 'test-error' ? 1 : selections.length);
    } finally { f.close(); }
  });
}

for (const scenario of ['ci-source', 'empty-mode', 'bad-mode', 'sentinel-mode', 'missing-cargo', 'missing-binaries', 'malformed', 'moved-workspace', 'moved-target', 'moved-build']) test(`reject ${scenario} before fixture allocation`, () => {
  const f = fixture(true);
  try {
    if (scenario === 'ci-source') delete f.env.CMA_NEXTEST_REUSE;
    if (scenario === 'empty-mode') f.env.CMA_NEXTEST_REUSE = '';
    if (scenario === 'sentinel-mode') f.env.CMA_NEXTEST_REUSE = 'unset';
    if (scenario === 'bad-mode') f.env.CMA_NEXTEST_REUSE = 'true';
    if (scenario === 'missing-cargo') rmSync(join(f.metadata, 'cargo-metadata.json'));
    if (scenario === 'missing-binaries') rmSync(join(f.metadata, 'binaries-metadata.json'));
    if (scenario === 'malformed') writeFileSync(join(f.metadata, 'cargo-metadata.json'), 'invalid');
    if (scenario === 'moved-workspace' || scenario === 'moved-target' || scenario === 'moved-build') {
      const path = join(f.metadata, scenario === 'moved-workspace' ? 'cargo-metadata.json' : 'binaries-metadata.json');
      const value = JSON.parse(readFileSync(path));
      if (scenario === 'moved-workspace') value.workspace_root = '/elsewhere';
      else value['rust-build-meta'][scenario === 'moved-build' ? 'build-directory' : 'target-directory'] = '/elsewhere';
      writeFileSync(path, JSON.stringify(value));
    }
    assert.throws(f.run);
    assert.deepEqual(f.calls(), []);
  } finally { f.close(); }
});
