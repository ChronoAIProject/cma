import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { appendFileSync, chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { setup, run, name, filter, archive } from './scanner-contract-test-support.mjs';

test('required real-engine mode refuses a missing binary instead of skipping', () => {
  const env = { ...process.env }; delete env.CMA_TEST_GITLEAKS;
  const result = spawnSync('bash', ['.github/secret-scanner-build-tests.sh', '--require-real-binary'], { env, encoding: 'utf8' });
  assert.notEqual(result.status, 0, result.stdout);
  assert.ok(!result.stdout.includes('skip - real-binary'));
});

test('installer removes its owned directory after download failure', t => {
  const root = mkdtempSync(join(tmpdir(), 'scanner-install-failure-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const bin = join(root, 'bin'), temp = join(root, 'temp'); mkdirSync(bin); mkdirSync(temp);
  const curl = join(bin, 'curl'); writeFileSync(curl, '#!/bin/sh\nexit 19\n'); chmodSync(curl, 0o700);
  const result = spawnSync('bash', ['.github/scripts/install-image-gitleaks.sh'], {
    env: { ...process.env, PATH: bin + ':' + process.env.PATH, RUNNER_TEMP: temp, GITHUB_PATH: join(root, 'path') }, encoding: 'utf8' });
  assert.notEqual(result.status, 0);
  assert.deepEqual(readdirSync(temp), []);
});

const target = s => s.inventory['rust-suites']['cma::bin/cma'];
for (const reuse of [false, true]) test(`verified engine runs four CLI cases and the exact Rust case once in ${reuse ? 'archive' : 'source'} mode`, t => {
  const s = setup(t, reuse), result = run(s);
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.calls.length, 2);
  const common = reuse ? ['--binaries-metadata', join(s.metadata, 'binaries-metadata.json'), '--cargo-metadata', join(s.metadata, 'cargo-metadata.json')] : ['--workspace', '--locked'];
  const selection = ['--run-ignored', 'only', '-E', filter];
  assert.deepEqual(result.calls[0].args, ['nextest', 'list', ...common, ...selection, '--message-format', 'json']);
  assert.deepEqual(result.calls[1].args, ['nextest', 'run', ...common, '--no-tests', 'fail', ...selection, '--test-threads', '1', '--retries', '0', '--no-fail-fast', '--success-output', 'final']);
  assert.ok(result.calls.every(call => call.incremental === '0'));
  const receipt = JSON.parse(result.stdout.trim().split('\n').at(-1));
  assert.equal(receipt.rust_case, name); assert.equal(receipt.test_exit, 0);
  assert.equal(receipt.engine.archive_sha256, archive); assert.equal(receipt.cli_cases_passed.length, 4);
  assert.equal(result.calls[1].tool, receipt.engine.executable);
  assert.notEqual(result.calls[1].tool, '/untrusted/caller/tool');
  assert.equal(receipt.owned_installation_removed, true);
  assert.equal(existsSync(receipt.engine.executable), false);
  assert.deepEqual(readdirSync(s.env.RUNNER_TEMP), []);
});
for (const [label, mutate] of [
  ['absent exact test', s => { target(s).testcases = {}; }],
  ['unselected case', s => { target(s).testcases[name]['filter-match'].status = 'mismatch'; }],
  ['ordinary case', s => { target(s).testcases[name].ignored = false; }],
  ['string ignored flag', s => { target(s).testcases[name].ignored = 'true'; }],
  ['unknown selection state', s => { target(s).testcases[name]['filter-match'].status = 'unknown'; }],
  ['live GitHub case selected', s => { target(s).testcases['profile_authoring::tests::live_trureturing_repository_resolution'] = target(s).testcases[name]; }],
  ['second binary selected', s => { s.inventory['rust-suites'].other = { testcases: { [name]: target(s).testcases[name] } }; }],
  ['wrong package', s => { target(s)['package-name'] = 'other'; }],
  ['wrong binary', s => { target(s)['binary-name'] = 'other'; }],
  ['wrong kind', s => { target(s).kind = 'lib'; }],
  ['missing binary', s => { target(s)['binary-path'] = '/missing/scanner-test'; }],
  ['bad JSON', s => { s.inventory = 'private-canary'; }],
  ['duplicate case keys', s => { const row = JSON.stringify(target(s).testcases[name]); s.inventory = JSON.stringify(s.inventory).replace(JSON.stringify(name)+':'+row, JSON.stringify(name)+':'+row+','+JSON.stringify(name)+':'+row); }],
]) test(`preflight rejects ${label} without installing or running`, t => {
  const s = setup(t); mutate(s); const result = run(s);
  assert.equal(result.status, 1);
  assert.ok(result.calls.every(call => call.args[1] === 'list'));
  assert.equal(existsSync(join(s.root, 'install-calls.jsonl')), false);
  assert.ok(!result.stdout.includes('private-canary') && !result.stderr.includes('private-canary'));
  assert.deepEqual(readdirSync(s.env.RUNNER_TEMP), []);
});
for (const failure of ['downloadFail', 'checksumFail', 'wrongVersion', 'engineFail', 'unredacted']) {
  test(`${failure} refuses and cleans the owned installation without a Rust run`, t => {
    const s = setup(t); s.control[failure] = true; const result = run(s);
    assert.notEqual(result.status, 0);
    assert.deepEqual(result.calls.map(call => call.args[1]), ['list']);
    assert.deepEqual(readdirSync(s.env.RUNNER_TEMP), []);
    assert.ok(!result.stdout.includes('SERVICE_API_KEY') && !result.stderr.includes('SERVICE_API_KEY'));
  });
}
for (const phase of ['list', 'run']) test(`${phase} failure keeps its status without retry`, t => {
  const s = setup(t); s.control[phase + 'Exit'] = 37; const result = run(s);
  assert.equal(result.status, 37);
  assert.deepEqual(result.calls.map(call => call.args[1]), phase === 'list' ? ['list'] : ['list', 'run']);
  assert.deepEqual(readdirSync(s.env.RUNNER_TEMP), []);
});
for (const text of ['skip - real-binary checks require a tool', 'SCANNER_CASE_PASS=clean values']) {
  test('a successful CLI exit without all four real cases cannot qualify', t => {
    const s = setup(t); writeFileSync(join(s.root, '.github/secret-scanner-build-tests.sh'), 'printf "%s\\n" '+JSON.stringify(text)+'\n');
    const result = run(s); assert.equal(result.status, 1);
    assert.deepEqual(result.calls.map(call => call.args[1]), ['list']);
    assert.deepEqual(readdirSync(s.env.RUNNER_TEMP), []);
  });
}
test('CI without archive and missing metadata fail before Cargo', t => {
  for (const mode of ['unset', 'missing']) {
    const s = setup(t);
    if (mode === 'unset') delete s.env.CMA_NEXTEST_REUSE;
    else rmSync(join(s.metadata, 'binaries-metadata.json'));
    const result = run(s); assert.notEqual(result.status, 0); assert.deepEqual(result.calls, []);
    assert.equal(existsSync(join(s.root, 'install-calls.jsonl')), false);
  }
});
test('caller cannot override the test selector or engine pin', t => {
  const result = run(setup(t), ['--test', 'other']);
  assert.equal(result.status, 1); assert.deepEqual(result.calls, []);
});

for (const field of ['version', 'archive_sha256', 'binary_sha256', 'artifact_platform', 'executable']) {
  test(`installer receipt with wrong ${field} is refused before qualification`, t => {
    const s = setup(t);
    appendFileSync(join(s.scripts, 'install-image-gitleaks.sh'), `
python3 - "$install_dir/identity.json" <<'PY'
import json,sys
from pathlib import Path
path=Path(sys.argv[1])
receipt=json.loads(path.read_text())
receipt[${JSON.stringify(field)}]='wrong'
path.write_text(json.dumps(receipt))
PY
`);
    const result = run(s);
    assert.equal(result.status, 1);
    assert.deepEqual(result.calls.map(call => call.args[1]), ['list']);
    assert.deepEqual(readdirSync(s.env.RUNNER_TEMP), []);
  });
}
for (const field of ['changeTest', 'changeEngine', 'changeHead']) test(`${field} during execution invalidates the receipt`, t => {
  const s = setup(t); s.control[field] = true;
  const result = run(s);
  assert.equal(result.status, 1);
  assert.deepEqual(result.calls.map(call => call.args[1]), ['list', 'run']);
  assert.deepEqual(readdirSync(s.env.RUNNER_TEMP), []);
});
test('legacy installer preserves its GITHUB_PATH interface without GITHUB_OUTPUT', t => {
  const s = setup(t);
  writeFileSync(join(s.root, 'control.json'), '{}');
  const pathFile = join(s.root, 'path');
  const result = spawnSync('bash', [join(s.scripts, 'install-image-gitleaks.sh')], {
    env: { ...s.env, GITHUB_PATH: pathFile }, encoding: 'utf8', timeout: 10000 });
  assert.equal(result.status, 0, result.stderr);
  const directory = readFileSync(pathFile, 'utf8').trim();
  assert.ok(directory.startsWith(s.env.RUNNER_TEMP + '/image-gitleaks.'));
  assert.ok(existsSync(join(directory, 'gitleaks')));
  assert.equal(JSON.parse(readFileSync(join(directory, 'identity.json'))).archive_sha256, archive);
});
