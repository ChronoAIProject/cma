import test from 'node:test';
import assert from 'node:assert/strict';
import { hash } from '../lib/checks.mjs';
import { haCandidateHash, validateHaEvidence } from '../lib/ha-evidence.mjs';
import { deterministicCases } from '../scenarios/contract.mjs';
import { haReplicaTargets } from '../scenarios/scaling.mjs';
import { contractProof, fakeDriver, oracle } from './fixtures.mjs';

const candidate = { sourceSha: 'a'.repeat(40), amImageDigest: `sha256:${hash('am-image')}`,
  egImageDigest: `sha256:${hash('eg-image')}`, runtimeImageDigest: `sha256:${hash('runtime-image')}`,
  configHash: hash('configuration'), protocolHash: hash('protocol'), environmentHash: hash('environment') };
const candidateHash = haCandidateHash(candidate);
const common = () => ({ candidateHash, runHash: hash('run'), evidenceHash: hash('observation'),
  startedAtMs: 1000, finishedAtMs: 10000, completed: true, cancelled: false, attempt: 1, firstRunOutcome: 'PASS' });
const correctness = () => ({ ...contractProof(), oracle: oracle(), wrongRoutes: 0, streamGaps: 0 });
const timing = () => ({ lossStartedAtMs: 2000, sustainedSuccessAtMs: 3000, recoveryMs: 1000, sustainedSuccess: true });
const trial = (role, faultKind, index) => ({ ...common(), ...correctness(), ...timing(), role, faultKind,
  trialHash: hash(`${role}:${faultKind}:${index}`), faultStartedAtMs: 2000, faultObservedAtMs: 2100,
  processDeathObserved: true, lostProcessHash: hash('process-a'), lostIncarnationHash: hash(`incarnation:${index}`),
  faultEvidenceHash: hash(`fault:${role}:${index}`), nodeLossObserved: true, lostNodeHash: hash('node'),
  drainStartedAtMs: 2000, leaseJustRenewed: true, affectedAcceptedHashes: [hash('command')],
  minimumConnectedUsers: 100, minimumOpenStreams: 100, actualReadsPerSecond: 10, actualTurnsPerMinute: 20 });

async function packet() {
  const deployment = () => ({ ...common(), pdbApiVersion: 'policy/v1', effectiveSurvivalFloor: 1,
    pdbEffective: true, independentlyServing: true, separateNodes: true, readinessWithdrawal: true,
    signedPreStop: true, boundedTermination: true, surgeBounded: true, terminatingBounded: true,
    sparePlacementCapacity: true });
  const scaling = [];
  for (const [index, target] of haReplicaTargets.entries()) {
    const startedAtMs = 10000 + index * 3000;
    scaling.push({ ...await fakeDriver().replicaStep({ ...target, mode: 'manual' }), ...common(),
      startedAtMs, finishedAtMs: startedAtMs + 2000, lossStartedAtMs: startedAtMs,
      sustainedSuccessAtMs: startedAtMs + 1000 });
  }
  return { ...common(), schema: 'cma-ha-evidence-v1', candidate: structuredClone(candidate),
    startedAtMs: 0, finishedAtMs: 100000, rawEvidenceHash: hash('raw-private-packet'),
    declaredCounts: { am: 101, eg: 10 }, scalingMode: 'manual', scaling,
    trials: [...Array.from({ length: 101 }, (_, i) => trial('am', i % 2 ? 'graceful' : 'process', i)),
      ...Array.from({ length: 10 }, (_, i) => trial('eg', i < 5 ? 'process' : 'node', i))],
    contracts: [...deterministicCases, 'cold_cache', 'warm_cache', 'rollout_surge']
      .map((name) => ({ ...common(), ...correctness(), name })),
    disruptions: ['am', 'eg'].flatMap((role) => ['node', 'node_drain'].map((kind) => ({ ...trial(role, kind, 200),
      homeHashBefore: hash('home'), homeHashAfter: hash('home'), manualDatabaseRepair: false,
      pendingOverFiveMinutes: 0, nPlusOneCapacityMaintained: true }))),
    repairs: [0, 1].map((index) => ({ ...common(), ...correctness(), ...timing(), trialHash: hash(`repair:${index}`),
      forwardRepair: true, historyAvailable: true, newTurnAvailable: true, databaseRollback: false,
      homeDeleted: false, asset404: 0, reconnectContractProven: true })),
    prerequisites: { am: deployment(), eg: deployment(),
      atlas: { ...common(), cpuHeadroom: 0.3, connectionHeadroom: 0.4, storageHeadroom: 0.5,
        allClientsAccounted: true, surgeAndTerminatingIncluded: true, otherConsumersIncluded: true,
        poolBudgetHash: hash('pool-budget') },
      workers: { ...common(), passed: true, inventoryHash: hash('worker-inventory') },
      compatibility: { ...common(), coordinatedProtocol: true, retainedRuntimeCompatible: true,
        legacyTransitionNoOverlap: true } },
  };
}

test('pure HA validator accepts a complete candidate-bound packet without mutating inputs', async () => {
  const value = await packet(); const before = structuredClone(value);
  assert.deepEqual(validateHaEvidence(candidate, value), { status: 'PASS', failures: [],
    counts: { am: 101, eg: 10, scaling: 4, contracts: 23, disruptions: 4, repairs: 2 } });
  assert.deepEqual(value, before);
  assert.deepEqual(validateHaEvidence(candidate, value), validateHaEvidence(candidate, before));
  for (const step of value.scaling) step.scalingMode = 'hpa';
  value.scalingMode = 'hpa';
  assert.equal(validateHaEvidence(candidate, value).status, 'PASS');
});

test('candidate identity binds source, all images, configuration, protocol and environment', async () => {
  for (const field of Object.keys(candidate)) {
    const value = await packet(); value.candidate[field] = field === 'sourceSha' ? 'b'.repeat(40)
      : field.endsWith('Digest') ? `sha256:${hash('other-image')}` : hash('other-identity');
    assert.equal(validateHaEvidence(candidate, value).failures[0].code, 'ha_candidate_mismatch');
    const missing = { ...candidate }; delete missing[field];
    assert.equal(validateHaEvidence(missing, value).status, 'BLOCKED');
  }
  assert.equal(validateHaEvidence(null, {}).status, 'BLOCKED');
  assert.equal(validateHaEvidence(candidate, null).status, 'BLOCKED');
});

test('missing, cancelled, incomplete and retry-only packets never pass', async () => {
  for (const patch of [{ schema: 'other' }, { completed: false }, { cancelled: true }, { attempt: 2 },
    { firstRunOutcome: 'FAIL' }, { rawEvidenceHash: null }, { runHash: null }, { candidateHash: null },
    { evidenceHash: null }, { startedAtMs: '0' }, { finishedAtMs: 0 }]) {
    const value = Object.assign(await packet(), patch);
    assert.notEqual(validateHaEvidence(candidate, value).status, 'PASS', JSON.stringify(patch));
  }
  for (const section of ['trials', 'scaling', 'contracts', 'disruptions', 'repairs', 'prerequisites']) {
    const value = await packet(); delete value[section];
    assert.notEqual(validateHaEvidence(candidate, value).status, 'PASS', section);
  }
});

test('every sample requires the same run and candidate, first-run outcome, window and evidence', async () => {
  for (const section of ['trials', 'scaling', 'contracts', 'disruptions', 'repairs']) {
    for (const patch of [{ candidateHash: hash('other') }, { runHash: hash('other') }, { completed: false },
      { cancelled: true }, { attempt: 2 }, { firstRunOutcome: 'FAIL' }, { evidenceHash: null },
      { startedAtMs: -1 }, { finishedAtMs: 100001 }, { processHashes: [hash('one')] },
      { clientHashes: [hash('one'), hash('one')] }, { productionPath: false }, { passed: false }]) {
      const value = await packet(); Object.assign(value[section][0], patch);
      assert.notEqual(validateHaEvidence(candidate, value).status, 'PASS', `${section}:${JSON.stringify(patch)}`);
    }
  }
});

test('fault recovery starts at loss, retains incarnation and affected receipts, and includes drain time', async () => {
  for (const patch of [{ recoveryMs: 999 }, { sustainedSuccessAtMs: 10001 }, { lossStartedAtMs: 999 },
    { sustainedSuccess: false }, { faultStartedAtMs: 1000 }, { faultObservedAtMs: 4000 },
    { processDeathObserved: false }, { lostProcessHash: hash('absent') }, { lostIncarnationHash: null },
    { faultEvidenceHash: null }, { leaseJustRenewed: false }, { affectedAcceptedHashes: [] },
    { affectedAcceptedHashes: [hash('unreconciled')] }, { minimumConnectedUsers: 99 },
    { minimumOpenStreams: 99 }, { actualReadsPerSecond: 9 }, { actualTurnsPerMinute: 19 }]) {
    const value = await packet(); Object.assign(value.trials[0], patch);
    assert.notEqual(validateHaEvidence(candidate, value).status, 'PASS', JSON.stringify(patch));
  }
  const slow = await packet(); Object.assign(slow.trials[0], { finishedAtMs: 40000,
    sustainedSuccessAtMs: 32001, recoveryMs: 30001 });
  assert.ok(validateHaEvidence(candidate, slow).failures.some((f) => f.code === 'ha_recovery_failed'));
  Object.assign(slow.trials[0], { sustainedSuccessAtMs: 32000, recoveryMs: 30000 });
  assert.equal(validateHaEvidence(candidate, slow).status, 'PASS');
  for (const [index, patch] of [[1, { drainStartedAtMs: 1999 }], [106, { nodeLossObserved: false }],
    [106, { lostNodeHash: null }]]) {
    const value = await packet(); Object.assign(value.trials[index], patch);
    assert.notEqual(validateHaEvidence(candidate, value).status, 'PASS');
  }
});

test('all trials including extra attempts must pass with unique identities and exact declared counts', async () => {
  for (const mutate of [
    (v) => { v.trials.pop(); },
    (v) => { v.trials[1].trialHash = v.trials[0].trialHash; },
    (v) => { v.trials[0].role = 'unknown'; },
    (v) => { v.trials[0].faultKind = 'api_delete'; },
    (v) => { v.trials[1].faultKind = 'process'; },
    (v) => { v.declaredCounts.am = 100; },
    (v) => { v.trials.push({ ...trial('am', 'process', 999), firstRunOutcome: 'FAIL' }); },
  ]) {
    const value = await packet(); mutate(value);
    assert.notEqual(validateHaEvidence(candidate, value).status, 'PASS');
  }
});

test('reconciliation rejects wrong routes, stale writes, missing work and duplicated effects', async () => {
  for (const section of ['trials', 'scaling', 'contracts', 'disruptions', 'repairs']) {
    for (const field of ['wrongRoutes', 'staleWrites', 'acceptedLost', 'duplicateEffects', 'streamGaps']) {
      const value = await packet(); value[section][0][field] = 1;
      assert.notEqual(validateHaEvidence(candidate, value).status, 'PASS');
    }
    for (const patch of [{ effects: 2 }, { executions: 0 }, { terminal: false }, { correctOwner: false }]) {
      const value = await packet(); Object.assign(value[section][0].oracle.commands[0], patch);
      assert.notEqual(validateHaEvidence(candidate, value).status, 'PASS');
    }
  }
});

test('scaling requires the exact ordered Ready sequence and full serving workload', async () => {
  for (const mutate of [
    (v) => { v.scaling.pop(); },
    (v) => { v.scaling.push(v.scaling[0]); },
    (v) => { v.scaling.reverse(); },
    (v) => { v.scaling[1].startedAtMs = 10000; },
    (v) => { v.scaling[2].am.ready = 2; },
    (v) => { v.scaling[2].eg.ready = 5; },
    (v) => { v.scaling[2].am.servingReplicaHashes = []; },
    (v) => { v.scaling[2].actualReadsPerSecond = 0; },
    (v) => { v.scalingMode = 'unknown'; },
    (v) => { v.contracts.pop(); },
    (v) => { v.contracts[1].name = v.contracts[0].name; },
  ]) {
    const value = await packet(); mutate(value);
    assert.notEqual(validateHaEvidence(candidate, value).status, 'PASS');
  }
});

test('PDB, topology, worker inventory, protocol and Atlas evidence remain required', async () => {
  for (const section of ['am', 'eg', 'atlas', 'workers', 'compatibility']) {
    const value = await packet(); delete value.prerequisites[section];
    assert.notEqual(validateHaEvidence(candidate, value).status, 'PASS');
  }
  for (const [section, field, invalid] of [['am', 'pdbEffective', false], ['eg', 'separateNodes', false],
    ['am', 'effectiveSurvivalFloor', 0], ['atlas', 'cpuHeadroom', 0.29], ['atlas', 'connectionHeadroom', 0.29],
    ['atlas', 'storageHeadroom', 0.29], ['atlas', 'otherConsumersIncluded', false],
    ['atlas', 'surgeAndTerminatingIncluded', false], ['workers', 'passed', false],
    ['compatibility', 'retainedRuntimeCompatible', false], ['compatibility', 'legacyTransitionNoOverlap', false]]) {
    const value = await packet(); value.prerequisites[section][field] = invalid;
    assert.notEqual(validateHaEvidence(candidate, value).status, 'PASS');
  }
});

test('node disruptions and compatible repairs cannot be replaced by process trials or unsafe recovery', async () => {
  for (const mutate of [
    (v) => { v.disruptions.pop(); },
    (v) => { v.disruptions[0].faultKind = 'process'; },
    (v) => { v.disruptions[0].homeHashAfter = hash('changed-home'); },
    (v) => { v.disruptions[0].manualDatabaseRepair = true; },
    (v) => { v.disruptions[0].nPlusOneCapacityMaintained = false; },
    (v) => { v.repairs.pop(); },
    (v) => { v.repairs[0].databaseRollback = true; },
    (v) => { v.repairs[0].homeDeleted = true; },
    (v) => { v.repairs[0].reconnectContractProven = false; },
    (v) => { v.repairs[0].historyAvailable = false; },
  ]) {
    const value = await packet(); mutate(value);
    assert.notEqual(validateHaEvidence(candidate, value).status, 'PASS');
  }
});
