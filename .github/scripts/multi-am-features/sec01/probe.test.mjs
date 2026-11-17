import test from 'node:test';
import assert from 'node:assert/strict';
import {spawnSync} from 'node:child_process';
import {mkdtempSync, writeFileSync, rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join} from 'node:path';

const script = new URL('./probe.py', import.meta.url).pathname;
function run(packet) {
  const child = spawnSync('python3', [script], {input: JSON.stringify(packet), encoding: 'utf8'});
  return {status: child.status, value: JSON.parse(child.stdout)};
}

test('bounded file primitive returns hash and no file content', () => {
  const directory = mkdtempSync(join(tmpdir(), 'sec01-'));
  try {
    const path = join(directory, 'owner-canary');
    writeFileSync(path, 'synthetic-private-marker');
    const value = run({schema: 'sec01-probe-v1', nonce: 'nonce-1234567890', probes: [{
      id: 'owner', kind: 'file', path, directory, synthetic: true,
    }]});
    assert.equal(value.status, 0);
    assert.equal(value.value.schema, 'sec01-observation-v1');
    const observation = value.value.observations[0];
    const read = observation.operations.find(row => row.operation === 'read');
    assert.equal(read.succeeded, true);
    assert.equal(read.bytes, 24);
    assert.equal(read.sha256.length, 64);
    assert.ok(!JSON.stringify(value.value).includes('synthetic-private-marker'));
  } finally { rmSync(directory, {recursive: true, force: true}); }
});

test('invalid and oversized packets fail closed without echoing input', () => {
  const invalid = spawnSync('python3', [script], {input: JSON.stringify({schema: 'wrong', secret: 'do-not-print'}), encoding: 'utf8'});
  assert.equal(invalid.status, 2);
  assert.equal(invalid.stdout.trim(), '{"error": "invalid_probe_input"}');
  const oversized = spawnSync('python3', [script], {input: 'x'.repeat(65537), encoding: 'utf8'});
  assert.equal(oversized.status, 2);
  assert.equal(oversized.stdout.trim(), '{"error": "invalid_probe_input"}');
});
