import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm, writeFile, realpath } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
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
  const records = {
    admissions: [{ ...authority, commandId, accepted: true, observedSequence: 10 }],
    authority: [{ ...authority, fromSequence: 0, throughSequence: 20 }],
    durable: [{ ...authority, observedSequence: 12 }],
    receiver: [{ ...authority, observedSequence: 13 }],
  };
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
    [(r) => r.receiver.push({ ...r.receiver[0] }), /duplicate_effect/],
    [(r) => { r.receiver[0].authorityEpoch = 2; }, /stale_epoch_write/],
    [(r) => { r.durable[0].ownerHash = hash('other-owner'); }, /stale_epoch_write/],
    [(r) => r.durable.push({ ...r.durable[0] }), /duplicate_execution/],
    [(r) => r.receiver.push({ ...authority, commandHash: hash('other-command') }), /oracle_unexpected_command/],
  ]) {
    await assert.rejects(readLocalOutcomeOracle(await fixture(t, modify)), error);
  }
});

test('agreement between stale execution and receiver records cannot establish current authority', async (t) => {
  const input = await fixture(t, (records) => {
    records.durable[0].authorityEpoch = 2;
    records.receiver[0].authorityEpoch = 2;
  });
  await assert.rejects(readLocalOutcomeOracle(input, {
    readCommand: async (id) => ({ receipt: { command_id: id }, status: 'completed' }),
  }), /stale_epoch_write/);
});

test('independently observed takeover permits a new owner and epoch', async (t) => {
  const successor = { ...authority, ownerHash: hash('successor-owner'), authorityEpoch: 4 };
  const input = await fixture(t, (records) => {
    records.authority[0].throughSequence = 11;
    records.authority.push({ ...successor, fromSequence: 12, throughSequence: 20 });
    records.durable[0] = { ...successor, observedSequence: 12 };
    records.receiver[0] = { ...successor, observedSequence: 13 };
  });
  const result = await readLocalOutcomeOracle(input);
  assert.equal(result.commands[0].correctOwner, true);
  assert.equal(result.commands[0].staleWrites, 0);
  const stale = await fixture(t, (records) => {
    records.authority[0].throughSequence = 11;
    records.authority.push({ ...successor, fromSequence: 12, throughSequence: 20 });
  });
  await assert.rejects(readLocalOutcomeOracle(stale), /stale_epoch_write/);
});

test('missing or ambiguous authority observations fail closed', async (t) => {
  for (const [modify, error] of [
    [(r) => { r.authority.length = 0; }, /oracle_authority_timeline_missing/],
    [(r) => { r.authority[0].throughSequence = 12; }, /oracle_authority_at_observation_missing/],
    [(r) => { r.admissions[0].authorityEpoch = 2; }, /oracle_admission_authority_mismatch/],
    [(r) => { r.receiver[0].observedSequence = 9; }, /oracle_observation_before_admission/],
    [(r) => r.authority.push({ ...authority, authorityEpoch: 4, fromSequence: 20, throughSequence: 30 }),
      /oracle_authority_interval_discontinuous/],
    [(r) => r.authority.push({ ...authority, authorityEpoch: 2, fromSequence: 21, throughSequence: 30 }),
      /oracle_authority_regressed/],
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


test('the pinned connected workload and independent admissions must name the same commands', async (t) => {
  const input = await fixture(t);
  const sourceSha = 'a'.repeat(40);
  const origin = config.endpoints.am;
  const path = join(dirname(input.captures.admissions.path), 'workload.json');
  const driverConfig = { ...config, sourceHash: hash(sourceSha),
    outcome: { ...input, origin, cluster: { kubeconfig: '/tmp/synthetic-kubeconfig', namespaceHash: hash('local') } } };
  const command = async (_file, args) => ({
    stdout: args.includes('--minify') ? 'https://127.0.0.1:6443'
      : JSON.stringify({ metadata: { name: 'cma-scale', uid: 'local' } }),
  });
  const fetchImpl = async () => new Response(JSON.stringify({
    receipt: { command_id: commandId }, status: 'completed',
  }));
  const writeReport = async (commands) => {
    const evidence = { schema: 'cma-local-workload-v1', status: 'PASS', runHash, sourceSha,
      fixtureHash: config.fixtureHash, workloadHash: config.workloadHash,
      attempts: commands.map((commandHash) => ({ kind: 'turn', status: 'PASS', commandHash })) };
    const bytes = JSON.stringify({ schema: 'cma-scale-results-v1', environment: 'local', selection: 'connected-workload',
      scenarios: [{ scenario: 'connected_workload', status: 'PASS', evidence, evidence_hash: hash(JSON.stringify(evidence)) }] });
    await writeFile(path, bytes, { mode: 0o600 });
    driverConfig.outcome.workloadReport = { path, sha256: hash(bytes) };
  };
  await writeReport([commandHash]);
  const result = await verifyLocalOutcomes(driverConfig, { command, fetchImpl });
  assert.deepEqual(result.acceptedHashes, [commandHash]);
  await writeReport([commandHash, hash('omitted-independent-admission')]);
  await assert.rejects(verifyLocalOutcomes(driverConfig, { command, fetchImpl }), /oracle_workload_admissions_mismatch/);
  await writeReport([commandHash, commandHash]);
  await assert.rejects(verifyLocalOutcomes(driverConfig, { command, fetchImpl }), /oracle_workload_duplicate_command/);
  await writeReport([commandHash]);
  await writeFile(path, '{}');
  await assert.rejects(verifyLocalOutcomes(driverConfig, { command, fetchImpl }), /oracle_workload_changed/);
});
