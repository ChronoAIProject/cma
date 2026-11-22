import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, realpathSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { hash } from './contract.mjs';
import { fixture } from './fixtures/candidate.mjs';

const cli = fileURLToPath(new URL('../smoke-evidence.mjs', import.meta.url));
const fields = ['policyBytes', 'manifestBytes', 'beforeBytes', 'executionBytes', 'afterBytes', 'resultsBytes'];
function sandbox(t) {
  const dir = realpathSync(mkdtempSync(join(tmpdir(), 'smoke-evidence-')));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const input = fixture().encode();
  const paths = fields.map((field) => {
    const path = join(dir, field);
    writeFileSync(path, input[field], { mode: 0o600 });
    return path;
  });
  return { dir, input, paths, args: ['--strict', paths[0], input.policyHash, ...paths.slice(1)] };
}
function invoke(args, env = {}) {
  const output = spawnSync(process.execPath, [cli, ...args], { encoding: 'utf8', timeout: 5000,
    env: { PATH: '', ...env } });
  assert.equal(output.error, undefined);
  assert.equal(output.signal, null);
  assert.equal(output.stderr, '');
  return { ...output, result: JSON.parse(output.stdout) };
}
function snapshot(dir) {
  return Object.fromEntries(readdirSync(dir).sort().map((name) => [name, hash(readFileSync(join(dir, name)))]));
}

test('default invocation does not consume ambient evidence, credentials or hooks', (t) => {
  const s = sandbox(t), original = snapshot(s.dir);
  const trap = join(s.dir, 'unused-hook');
  writeFileSync(trap, 'throw new Error("ambient-hook-canary")', { mode: 0o600 });
  const output = invoke([], { CMA_E2E_FIXTURES: s.paths[1], CMA_E2E_OBSERVER: trap, CMA_SMOKE_POLICY: s.paths[0] });
  assert.equal(output.status, 0);
  assert.equal(output.result.reason, 'binding_disabled');
  rmSync(trap);
  assert.deepEqual(snapshot(s.dir), original);
});
test('strict CLI verifies private signed evidence and preserves every original file', (t) => {
  const s = sandbox(t), original = snapshot(s.dir);
  const output = invoke(s.args);
  assert.equal(output.status, 0);
  assert.equal(output.result.qualification, 'verified');
  assert.equal(output.result.rawSmokeStatus, 'passed');
  assert.equal(output.result.resultsHash, hash(s.input.resultsBytes));
  assert.ok(!output.stdout.includes('example.test'));
  assert.deepEqual(snapshot(s.dir), original);
});
for (const index of [1, 3, 4, 5, 6, 7]) {
  test(`strict CLI refuses missing input ${index} without hiding available smoke status`, (t) => {
    const s = sandbox(t), original = snapshot(s.dir);
    s.args[index] = join(s.dir, 'missing-private-path-canary');
    const output = invoke(s.args);
    assert.equal(output.status, 1);
    assert.equal(output.result.reason, 'file_unavailable');
    assert.equal(output.result.rawSmokeStatus, index === 7 ? 'unavailable' : 'passed');
    assert.ok(!output.stdout.includes('canary'));
    assert.deepEqual(snapshot(s.dir), original);
  });
}
test('strict CLI rejects an unpinned policy and invalid signature, preserving raw PASS', (t) => {
  const s = sandbox(t);
  const badPin = [...s.args]; badPin[2] = '0'.repeat(64);
  assert.equal(invoke(badPin).result.reason, 'untrusted_policy');
  const envelope = JSON.parse(s.input.afterBytes);
  envelope.signature = 'A'.repeat(86) + '==';
  writeFileSync(s.paths[4], JSON.stringify(envelope));
  const output = invoke(s.args);
  assert.equal(output.status, 1);
  assert.equal(output.result.rawSmokeStatus, 'passed');
  assert.equal(output.result.reason, 'invalid_signature');
});
for (const mode of ['symlink', 'parent-symlink', 'group-readable', 'directory', 'oversized', 'relative']) {
  test(`strict CLI refuses ${mode} evidence files`, (t) => {
    const s = sandbox(t);
    const alternate = join(s.dir, 'alternate');
    if (mode === 'symlink') { symlinkSync(s.paths[0], alternate); s.args[1] = alternate; }
    if (mode === 'parent-symlink') { symlinkSync(s.dir, alternate); s.args[1] = join(alternate, 'policyBytes'); }
    if (mode === 'group-readable') chmodSync(s.paths[0], 0o640);
    if (mode === 'directory') { mkdirSync(alternate, { mode: 0o700 }); s.args[1] = alternate; }
    if (mode === 'oversized') writeFileSync(s.paths[0], Buffer.alloc(65_537));
    if (mode === 'relative') s.args[1] = '.github/scripts/smoke-evidence.mjs';
    const output = invoke(s.args);
    assert.equal(output.status, 1);
    assert.equal(output.result.reason, 'file_unavailable');
    assert.equal(output.result.rawSmokeStatus, 'passed');
  });
}
test('strict CLI refuses private files inside the checkout', (t) => {
  const s = sandbox(t);
  const dir = mkdtempSync(fileURLToPath(new URL('./fixtures/temporary-', import.meta.url)));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  s.args[1] = join(dir, 'policy');
  writeFileSync(s.args[1], s.input.policyBytes, { mode: 0o600 });
  assert.equal(invoke(s.args).result.reason, 'file_unavailable');
});
test('unknown flags and extra arguments fail without reflecting input', () => {
  for (const args of [['--private-canary'], ['--help', 'private-canary'], ['--strict'], Array(9).fill('private-canary')]) {
    const output = invoke(args);
    assert.equal(output.status, 1);
    assert.equal(output.result.reason, 'invalid_invocation');
    assert.ok(!output.stdout.includes('canary'));
  }
});
test('help explains the external trust anchor and limits without loading inputs', () => {
  const output = spawnSync(process.execPath, [cli, '--help'], { encoding: 'utf8', timeout: 5000, env: { PATH: '' } });
  assert.equal(output.status, 0);
  assert.equal(output.stderr, '');
  assert.match(output.stdout, /POLICY_SHA256 must come from protected release authority/);
  assert.match(output.stdout, /not continuous stability, mutations or full release qualification/);
});
