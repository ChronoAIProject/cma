import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { chmodSync, copyFileSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import test from 'node:test';

const prefix = 'auth::keys::fixture_contract_tests::';
const registry = JSON.parse(readFileSync(new URL('./mongo-contract-exceptions.json', import.meta.url), 'utf8'));
const expected = Object.keys(registry).filter(name => name.startsWith(prefix));
const filter = `package(=cma) and binary(=cma) and test(~${prefix})`;
const selectedCase = () => ({ ignored: true, 'filter-match': { status: 'matches' } });
function setup(t, archive = false) {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'key-provider-runner-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scripts = join(root, '.github/scripts'), metadata = join(root, 'api/target/nextest');
  for (const dir of [scripts, metadata, join(root, 'bin')]) mkdirSync(dir, { recursive: true });
  for (const file of ['key-provider-contracts.py', 'backend-nextest.sh']) {
    copyFileSync(new URL(file, import.meta.url), join(scripts, file));
  }
  const entries = Object.fromEntries(expected.map(name => [name, registry[name]]));
  const inventory = { 'rust-suites': { 'cma::bin/cma': {
    'package-name': 'cma', 'binary-name': 'cma', 'binary-id': 'cma::bin/cma', kind: 'bin', status: 'listed',
    testcases: { ...Object.fromEntries(expected.map(name => [name, selectedCase()])),
      [`${prefix}process::tests::ordinary_cleanup`]: { ignored: false, 'filter-match': { status: 'mismatch', reason: 'ignored' } } },
  } } };
  const cargo = join(root, 'bin/cargo');
  writeFileSync(cargo, `#!${process.execPath}
const fs = require('node:fs');
const root = ${JSON.stringify(root)};
const args = process.argv.slice(2);
fs.appendFileSync(root + '/calls.jsonl', JSON.stringify({ args, cwd: process.cwd() }) + '\\n');
const control = JSON.parse(fs.readFileSync(root + '/control.json'));
if (args[0] !== 'nextest') process.exit(97);
if (args[1] === 'list') {
  process.stdout.write(fs.readFileSync(root + '/inventory.json'));
  process.exit(control.listExit || 0);
}
if (args[1] === 'run') process.exit(control.runExit || 0);
process.exit(98);
`);
  chmodSync(cargo, 0o700);
  const state = { root, scripts, metadata, inventory, entries, control: {},
    env: { PATH: `${join(root, 'bin')}:${dirname(process.execPath)}:${process.env.PATH}`, CI: archive ? 'true' : 'false',
      CARGO_INCREMENTAL: '0', ...(archive ? { CMA_NEXTEST_REUSE: '1' } : {}) } };
  writeFileSync(join(metadata, 'cargo-metadata.json'), JSON.stringify({ workspace_root: join(root, 'api'), target_directory: join(root, 'api/target') }));
  writeFileSync(join(metadata, 'binaries-metadata.json'), JSON.stringify({ 'rust-build-meta': { 'target-directory': join(root, 'api/target') } }));
  return state;
}
function run(s, args = []) {
  writeFileSync(join(s.scripts, 'mongo-contract-exceptions.json'), JSON.stringify(s.entries));
  writeFileSync(join(s.root, 'inventory.json'), typeof s.inventory === 'string' ? s.inventory : JSON.stringify(s.inventory));
  writeFileSync(join(s.root, 'control.json'), JSON.stringify(s.control));
  const result = spawnSync('python3', ['-B', join(s.scripts, 'key-provider-contracts.py'), ...args],
    { env: s.env, encoding: 'utf8', timeout: 10000 });
  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  let calls = [];
  try { calls = readFileSync(join(s.root, 'calls.jsonl'), 'utf8').trim().split('\n').map(JSON.parse); }
  catch (error) { assert.equal(error.code, 'ENOENT'); }
  return { ...result, calls };
}
const target = s => s.inventory['rust-suites']['cma::bin/cma'];
for (const archive of [false, true]) {
  test(`complete family runs once through the real helper in ${archive ? 'archive' : 'source'} mode`, t => {
    const s = setup(t, archive), result = run(s);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.calls.length, 2);
    const common = archive ? ['--binaries-metadata', join(s.metadata, 'binaries-metadata.json'),
      '--cargo-metadata', join(s.metadata, 'cargo-metadata.json')] : ['--workspace', '--locked'];
    const selection = ['--run-ignored', 'only', '-E', filter];
    assert.deepEqual(result.calls[0].args, ['nextest', 'list', ...common, ...selection, '--message-format', 'json']);
    assert.deepEqual(result.calls[1].args, ['nextest', 'run', ...common, '--no-tests', 'fail', ...selection,
      '--test-threads', '1', '--retries', '0', '--no-fail-fast', '--success-output', 'final']);
    assert.ok(result.calls.every(call => call.cwd === join(s.root, 'api')));
    for (const name of expected) assert.ok(result.stdout.includes(name));
  });
}
for (const [label, mutate] of [
  ['all omitted', s => { target(s).testcases = {}; }],
  ['one omitted', s => { delete target(s).testcases[expected[0]]; }],
  ['one unselected', s => { target(s).testcases[expected[0]]['filter-match'] = { status: 'mismatch' }; }],
  ['empty registry', s => { s.entries = {}; }],
  ['removed registration', s => { delete s.entries[expected[0]]; }],
  ['unregistered contract', s => { target(s).testcases[prefix + 'new_contract'] = selectedCase(); }],
  ['unregistered nested ignored test', s => { target(s).testcases[prefix + 'process::new_contract'] = selectedCase(); }],
  ['other test selected', s => { target(s).testcases['unrelated::test'] = selectedCase(); }],
  ['other binary selected', s => { s.inventory['rust-suites'].other = { testcases: { other: selectedCase() } }; }],
  ['ignore removed', s => { target(s).testcases[expected[0]].ignored = false; }],
  ['ignore string', s => { target(s).testcases[expected[0]].ignored = 'true'; }],
  ['unknown match status', s => { target(s).testcases[expected[0]]['filter-match'].status = 'unknown'; }],
  ['wrong package', s => { target(s)['package-name'] = 'other'; }],
  ['wrong binary', s => { target(s)['binary-name'] = 'other'; }],
  ['wrong kind', s => { target(s).kind = 'lib'; }],
  ['wrong binary ID', s => { target(s)['binary-id'] = 'other'; }],
  ['missing metadata', s => { delete s.inventory['rust-suites']; }],
  ['bad JSON', s => { s.inventory = 'private-canary-bad-json'; }],
  ['wrong registry classification', s => { s.entries[expected[0]] = { kind: 'helper', reason: 'synthetic' }; }],
]) test(`preflight rejects ${label} without running tests`, t => {
  const s = setup(t, true); mutate(s);
  const result = run(s);
  assert.equal(result.status, 1);
  assert.ok(result.calls.every(call => call.args[1] === 'list'));
  assert.ok(!result.stdout.includes('private-canary'));
  assert.ok(!result.stderr.includes('private-canary'));
});
for (const phase of ['list', 'run']) {
  test(`${phase} failure propagates once without retry`, t => {
    const s = setup(t); s.control[phase + 'Exit'] = 37;
    const result = run(s);
    assert.equal(result.status, 37);
    assert.deepEqual(result.calls.map(call => call.args[1]), phase === 'list' ? ['list'] : ['list', 'run']);
  });
}
for (const [label, mutate] of [
  ['CI without archive reuse', s => { delete s.env.CMA_NEXTEST_REUSE; }],
  ['missing archive metadata', s => { rmSync(join(s.metadata, 'binaries-metadata.json')); }],
  ['relocated archive', s => { writeFileSync(join(s.metadata, 'cargo-metadata.json'), JSON.stringify({ workspace_root: '/elsewhere', target_directory: '/elsewhere/target' })); }],
]) test(`${label} fails through the archive guard before cargo`, t => {
  const s = setup(t, true); mutate(s);
  const result = run(s);
  assert.notEqual(result.status, 0);
  assert.deepEqual(result.calls, []);
});
test('runner refuses caller-selected suites before any helper invocation', t => {
  const result = run(setup(t), ['--filter', 'other']);
  assert.equal(result.status, 1);
  assert.deepEqual(result.calls, []);
});
