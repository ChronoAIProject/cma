import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, writeFile, rm } from 'node:fs/promises';
import { join } from 'node:path';
import { GateError, count, finite, hash, reconcile, safeFailure, waitFor } from '../lib/checks.mjs';
import { localUrl, validateConfig, guardedFetch, pinnedModule } from '../lib/safety.mjs';
import { config, oracle, fakeTime } from './fixtures.mjs';

test('guards reject remote origins, encoded authority tricks and unsafe local scope', async () => {
  for (const value of ['https://cma.example', 'http://localhost:8080', 'http://127.0.0.1.evil.invalid',
    'http://127.0.0.1@evil.invalid', 'file:///tmp/example', 'http://127.0.0.1/#fragment', 'bad']) {
    assert.throws(() => localUrl(value));
  }
  assert.equal(localUrl('http://127.0.0.1:8080').port, '8080');
  assert.equal(localUrl('ws://[::1]:8080').protocol, 'ws:');
  assert.equal(validateConfig(config), config);
  for (const patch of [{ environment: 'production' }, { context: 'gke-cma' }, { namespace: 'chrono-bot' },
    { isolated: false }, { recoveryMs: 30001 }, { scaleTimeoutMs: 0 }, { hardwareHash: null },
    { endpoints: {} }, { endpoints: { am: 'http://127.0.0.1/path' } }]) {
    assert.throws(() => validateConfig({ ...config, ...patch }));
  }
  let called = false;
  const fetch = guardedFetch(async (url, options) => { called = true; return { url, options }; });
  await assert.rejects(fetch('https://remote.invalid'));
  await assert.rejects(fetch('http://127.0.0.1', { redirect: 'follow' }));
  assert.equal(called, false);
  assert.equal((await fetch('http://127.0.0.1')).options.redirect, 'error');
});

test('only exact pinned module bytes reach import', async () => {
  const dir = await mkdtemp(join(import.meta.dirname, '.pin-'));
  try {
    const path = join(dir, 'driver.mjs');
    await writeFile(path, 'export const value = 1;');
    await assert.rejects(pinnedModule(path, hash('wrong')));
    await assert.rejects(pinnedModule(path, 'invalid'));
    assert.equal(await pinnedModule(path, hash('export const value = 1;'), async () => 7), 7);
  } finally { await rm(dir, { recursive: true }); }
});

test('durable reconciliation rejects missing, duplicate, stale and incomplete evidence', () => {
  assert.equal(reconcile(oracle()).commands, 1);
  for (const patch of [{ executions: 0 }, { executions: 2 }, { effects: 0 }, { effects: 2 },
    { staleWrites: 1 }, { terminal: false }, { correctOwner: false }, { accepted: false }, { commandHash: 'raw' }]) {
    const value = oracle(); Object.assign(value.commands[0], patch);
    assert.throws(() => reconcile(value));
  }
  for (const patch of [{ commands: [] }, { independent: false }, { complete: false },
    { evidenceHash: null }, { acceptedHashes: [] }, { acceptedHashes: [hash('other')] }]) {
    assert.throws(() => reconcile({ ...oracle(), ...patch }));
  }
  const duplicate = oracle(); duplicate.commands.push(duplicate.commands[0]);
  assert.throws(() => reconcile(duplicate));
});

test('poll deadlines include slow reads; unsafe exceptions are redacted', async () => {
  const time = fakeTime(); let reads = 0;
  assert.equal(await waitFor(async () => ++reads, (n) => n === 2, { timeoutMs: 500, time }), 2);
  await assert.rejects(waitFor(async () => { await time.sleep(600); return true; }, Boolean,
    { timeoutMs: 500, time }), /deadline/);
  await assert.rejects(waitFor(async () => false, Boolean, { timeoutMs: 0, time }));
  assert.throws(() => finite(NaN)); assert.throws(() => count(0.5));
  assert.deepEqual(safeFailure(new Error('sensitive body')), { status: 'FAIL', code: 'adapter_or_transport_error' });
  assert.equal(safeFailure(new GateError('fixture_missing', true)).status, 'BLOCKED');
  assert.equal(safeFailure(new GateError('bad?value')).code, 'adapter_or_transport_error');
});
