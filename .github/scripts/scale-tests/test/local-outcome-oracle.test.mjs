import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm, writeFile, realpath } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { hash } from '../lib/checks.mjs';
import { readLocalOutcomeOracle, verifyLocalOutcomes } from '../lib/local-outcome-oracle.mjs';
import { config } from './fixtures.mjs';

const commandId = 'cmd_synthetic';
const commandHash = hash(commandId);
const ownerHash = hash('synthetic-owner');
const runHash = hash('synthetic-run');
const authority = { commandHash, ownerHash, runtimeGeneration: 2, authorityEpoch: 3, terminal: true };

async function fixture(t, transform = () => {}) {
  const directory = await realpath(await mkdtemp(join(tmpdir(), 'cma-outcome-')));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const records = { admissions: [{ commandId, commandHash, ownerHash, accepted: true }],
    durable: [{ ...authority }], receiver: [{ ...authority }] };
  transform(records);
  const captures = {};
  for (const [role, rows] of Object.entries(records)) {
    const path = join(directory, `${role}.json`);
    const bytes = JSON.stringify({ schema: 'cma-local-outcome-capture-v1', role, runHash,
      sourceHash: hash(role), complete: true, records: rows });
    await writeFile(path, bytes, { mode: 0o600 });
    captures[role] = { path, sha256: hash(bytes) };
  }
  return { runHash, captures };
}

test('oracle reconciles independently captured admission, execution and receiver observations', async (t) => {
  const input = await fixture(t);
  const result = await readLocalOutcomeOracle(input, {
    readCommand: async (id) => ({ receipt: { command_id: id }, status: 'completed' }),
  });
  assert.deepEqual(result.commands, [{ commandHash, accepted: true, executions: 1,
    effects: 1, staleWrites: 0, terminal: true, correctOwner: true }]);
  assert.equal(JSON.stringify(result).includes(commandId), false);
  await assert.rejects(readLocalOutcomeOracle(input, {
    readCommand: async () => ({ receipt: { command_id: 'wrong' }, status: 'completed' }),
  }), /oracle_live_terminal_mismatch/);
});

test('missing, duplicate, stale and foreign evidence cannot qualify', async (t) => {
  for (const [modify, error] of [
    [(r) => { r.receiver.length = 0; }, /accepted_effect_lost/],
    [(r) => r.receiver.push({ ...authority }), /duplicate_effect/],
    [(r) => { r.receiver[0].authorityEpoch = 2; }, /stale_epoch_write/],
    [(r) => { r.durable[0].ownerHash = hash('other-owner'); }, /stale_epoch_write/],
    [(r) => r.durable.push({ ...authority }), /duplicate_execution/],
    [(r) => r.receiver.push({ ...authority, commandHash: hash('other-command') }), /oracle_unexpected_command/],
  ]) {
    await assert.rejects(readLocalOutcomeOracle(await fixture(t, modify)), error);
  }
});

test('capture pins and distinct source bindings are mandatory', async (t) => {
  const input = await fixture(t);
  await assert.rejects(readLocalOutcomeOracle({ ...input, captures: { ...input.captures,
    receiver: input.captures.durable } }), /oracle_capture_source_reused/);
  await writeFile(input.captures.receiver.path, '{}');
  await assert.rejects(readLocalOutcomeOracle(input), /oracle_capture_changed/);
});

test('local readback verifies the isolated namespace and uses only the pinned local command route', async (t) => {
  const input = await fixture(t);
  const origin = config.endpoints.am;
  const namespaceUid = 'synthetic-namespace';
  const calls = [];
  const driverConfig = { ...config, outcome: { ...input, origin,
    cluster: { kubeconfig: '/tmp/synthetic-kubeconfig', namespaceHash: hash(namespaceUid) } } };
  const command = async (_file, args) => {
    calls.push(args);
    return { stdout: args.includes('--minify') ? 'https://127.0.0.1:6443'
      : JSON.stringify({ metadata: { name: 'cma-scale', uid: namespaceUid } }) };
  };
  const fetchImpl = async (url, options) => {
    assert.equal(url, `${origin}/api/v2/commands/${commandId}`);
    assert.equal(options.method, 'GET');
    assert.equal(options.redirect, 'error');
    return new Response(JSON.stringify({ receipt: { command_id: commandId }, status: 'completed' }), { status: 200 });
  };
  const result = await verifyLocalOutcomes(driverConfig, { command, fetchImpl });
  assert.equal(result.commands.length, 1);
  assert.ok(calls.every((args) => args.includes('kind-cma-scale') && args.includes('cma-scale')));
  await assert.rejects(verifyLocalOutcomes({ ...driverConfig, environment: 'production' },
    { command, fetchImpl }), /local_config_required/);
});
