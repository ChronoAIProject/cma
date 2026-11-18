import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';
import { hash } from '../lib/checks.mjs';
import { haCandidateHash, validateHaEvidence } from '../lib/ha-evidence.mjs';
import { deterministicCases } from '../scenarios/contract.mjs';
import { haReplicaTargets } from '../scenarios/scaling.mjs';
import { runSuite } from '../lib/suite.mjs';
import { haScenarioNames, saveReport } from '../lib/report.mjs';
import { config, contractProof, fakeDriver, fakeTime, oracle } from './fixtures.mjs';

const candidate = { sourceSha: 'a'.repeat(40), amImageDigest: `sha256:${hash('am-image')}`,
  webImageDigest: `sha256:${hash('web-image')}`,
  egImageDigest: `sha256:${hash('eg-image')}`, runtimeImageDigest: `sha256:${hash('runtime-image')}`,
  configHash: hash('configuration'), protocolHash: hash('protocol'), environmentHash: hash('environment') };
const candidateHash = haCandidateHash(candidate);
const common = () => ({ candidateHash, runHash: hash('run'), evidenceHash: hash('observation'),
  startedAtMs: 1000, finishedAtMs: 10000, completed: true, cancelled: false, attempt: 1, firstRunOutcome: 'PASS' });
const correctness = () => ({ ...contractProof(), oracle: oracle(), wrongRoutes: 0, streamGaps: 0 });
const timing = () => ({ lossStartedAtMs: 2000, sustainedSuccessAtMs: 3000, recoveryMs: 1000, sustainedSuccess: true });
const trial = (role, faultKind, index) => {
  const startedAtMs = 400000 + (role === 'am' ? index : 101 + index) * 40000;
  return { ...common(), ...correctness(), ...timing(), role, faultKind,
    request: { role, mode: faultKind, trial: index }, startedAtMs, finishedAtMs: startedAtMs + 35000,
    lossStartedAtMs: startedAtMs + 1000, sustainedSuccessAtMs: startedAtMs + 2000,
    trialHash: hash(`${role}:${faultKind}:${index}`), faultStartedAtMs: startedAtMs + 1000,
    faultObservedAtMs: startedAtMs + 1100, processDeathObserved: true, lostProcessHash: hash('process-a'),
    lostIncarnationHash: hash(`incarnation:${role}:${faultKind}:${index}`),
    faultEvidenceHash: hash(`fault:${role}:${faultKind}:${index}`), nodeLossObserved: true, lostNodeHash: hash('node'),
    drainStartedAtMs: startedAtMs + 1000, leaseJustRenewed: true, affectedAcceptedHashes: [hash('command')],
    minimumConnectedUsers: 100, minimumOpenStreams: 100, actualReadsPerSecond: 10, actualTurnsPerMinute: 20 };
};

async function packet() {
  const deployment = () => ({ ...common(), pdbApiVersion: 'policy/v1', effectiveSurvivalFloor: 1,
    pdbEffective: true, independentlyServing: true, separateNodes: true, readinessWithdrawal: true,
    signedPreStop: true, boundedTermination: true, surgeBounded: true, terminatingBounded: true,
    sparePlacementCapacity: true });
  const scaling = [];
  for (const [index, target] of haReplicaTargets.entries()) {
    const startedAtMs = 240000 + index * 30000;
    scaling.push({ ...await fakeDriver().replicaStep({ ...target, mode: 'manual' }), ...common(),
      startedAtMs, finishedAtMs: startedAtMs + 2000, lossStartedAtMs: startedAtMs,
      sustainedSuccessAtMs: startedAtMs + 1000 });
  }
  return { ...common(), schema: 'cma-ha-evidence-v1', candidate: structuredClone(candidate),
    startedAtMs: 0, finishedAtMs: 20000000, rawEvidenceHash: hash('raw-private-packet'),
    declaredCounts: { am: 101, eg: 10 }, scalingMode: 'manual', scaling,
    trials: [...Array.from({ length: 101 }, (_, i) => trial('am', i % 2 ? 'graceful' : 'process', i)),
      ...Array.from({ length: 10 }, (_, i) => trial('eg', i < 5 ? 'process' : 'node', i))],
    contracts: [...deterministicCases, 'cold_cache', 'warm_cache', 'rollout_surge']
      .map((name, index) => ({ ...common(), ...correctness(), name,
        startedAtMs: 1000 + index * 10000, finishedAtMs: 10000 + index * 10000 })),
    disruptions: ['am', 'eg'].flatMap((role) => ['node', 'node_drain'].map((kind, index) => ({ ...trial(role, kind, 200 + index),
      homeHashBefore: hash('home'), homeHashAfter: hash('home'), manualDatabaseRepair: false,
      pendingOverFiveMinutes: 0, nPlusOneCapacityMaintained: true }))),
    repairs: [0, 1].map((index) => ({ ...common(), ...correctness(), ...timing(), trialHash: hash(`repair:${index}`),
      startedAtMs: 14000000 + index * 2000000, finishedAtMs: 14002000 + index * 2000000,
      lossStartedAtMs: 14000500 + index * 2000000, sustainedSuccessAtMs: 14001500 + index * 2000000,
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
      { startedAtMs: -1 }, { finishedAtMs: 20000001 }, { processHashes: [hash('one')] },
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
  const slow = await packet(); Object.assign(slow.trials[0], {
    sustainedSuccessAtMs: slow.trials[0].lossStartedAtMs + 30001, recoveryMs: 30001 });
  assert.ok(validateHaEvidence(candidate, slow).failures.some((f) => f.code === 'ha_recovery_failed'));
  Object.assign(slow.trials[0], { sustainedSuccessAtMs: slow.trials[0].lossStartedAtMs + 30000, recoveryMs: 30000 });
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

const haConfig = () => ({ ...config, haCandidate: structuredClone(candidate), haScalingMode: 'manual' });
function haDriver(value) {
  let scaling = 0;
  return { ...fakeDriver(),
    async preflight() { return { ...await fakeDriver().preflight(), haCandidateHash: candidateHash }; },
    async contractCase(name) { return structuredClone(value.contracts.find((row) => row.name === name)); },
    async replicaStep() { return structuredClone(value.scaling[scaling++]); },
    async lossTrial({ role, trial }) { return structuredClone(value.trials.filter((row) => row.role === role)[trial]); },
    async haEvidence() { return structuredClone(value); },
    async measureCohort() { assert.fail('HA must not run the performance campaign'); },
    async connectUsers() { assert.fail('HA must not run the HPA performance experiment'); },
  };
}

test('HA-only suite records actual campaign evidence, validates it and preserves external NO_GO', async () => {
  const value = await packet(); const driver = haDriver(value);
  const report = await runSuite(haConfig(), driver, { scenario: 'ha', time: fakeTime() });
  assert.equal(report.ha.status, 'PASS'); assert.equal(report.status, 'NO_GO');
  assert.equal(report.track_status, 'INCOMPLETE'); assert.equal(driver.closed, true);
  assert.deepEqual(report.scenarios.slice(0, -1).map((row) => row.scenario), haScenarioNames);
  assert.ok(report.scenarios.slice(0, -1).every((row) => row.status === 'PASS'));
  assert.equal(report.scenarios.at(-1).code, 'external_qualification_integration_pending');
  assert.equal(report.ha.records.length, 144);
  const recorded = report.ha.records.find((row) => row.trialHash === value.trials[0].trialHash);
  assert.deepEqual(recorded.request, { role: 'am', mode: 'process', trial: 0 });
  assert.equal(recorded.lossStartedAtMs, value.trials[0].lossStartedAtMs);
  assert.equal(recorded.lostIncarnationHash, value.trials[0].lostIncarnationHash);
  assert.deepEqual(recorded.affectedAcceptedHashes, value.trials[0].affectedAcceptedHashes);
  assert.equal(recorded.reconciliation.status, 'PASS');
  assert.equal(report.performance.length, 0);
});

test('HA suite aborts on first failed trial, retains its outcome and closes the driver', async () => {
  const value = await packet(); value.trials[0].recoveryMs = 30001;
  value.trials[0].firstRunOutcome = 'FAIL'; value.trials[0].privateBody = 'PRIVATE_SENTINEL';
  const driver = haDriver(value);
  driver.haEvidence = async () => assert.fail('must stop after a failed trial');
  const report = await runSuite(haConfig(), driver, { scenario: 'ha', time: fakeTime() });
  assert.equal(report.ha.status, 'FAIL'); assert.equal(report.cleanup.completed, true);
  assert.equal(report.ha.records.length, 28);
  assert.equal(report.ha.records.at(-1).firstRunOutcome, 'FAIL');
  assert.equal(report.scenarios.find((row) => row.scenario === 'eg_10_trials').status, 'BLOCKED');
  assert.ok(!JSON.stringify(report).includes('PRIVATE_SENTINEL'));
});

test('HA runner blocks missing prerequisites and cannot substitute another campaign packet', async () => {
  const value = await packet(); delete value.prerequisites.atlas;
  const driver = haDriver(value);
  const report = await runSuite(haConfig(), driver, { scenario: 'ha', time: fakeTime() });
  assert.equal(report.ha.status, 'BLOCKED');
  assert.ok(report.ha.failures.some((failure) => failure.section === 'atlas'));
  for (const mutate of [(v) => { v.trials[0].trialHash = hash('replacement'); },
    (v) => { v.trials.push({ ...v.trials[0], firstRunOutcome: 'FAIL' }); }]) {
    const original = await packet(); const changed = structuredClone(original); mutate(changed);
    const driver = haDriver(original); driver.haEvidence = async () => changed;
    const report = await runSuite(haConfig(), driver, { scenario: 'ha', time: fakeTime() });
    assert.equal(report.scenarios.find((row) => row.scenario === 'ha_evidence').code, 'ha_recorded_observations_changed');
    assert.equal(report.ha.status, 'FAIL');
  }
  const leaking = haDriver(await packet()); leaking.close = async () => { throw new Error('PRIVATE_SENTINEL'); };
  const cleanup = await runSuite(haConfig(), leaking, { scenario: 'ha', time: fakeTime() });
  assert.equal(cleanup.status, 'NO_GO'); assert.equal(cleanup.cleanup.completed, false);
  assert.equal(cleanup.scenarios.find((row) => row.scenario === 'cleanup').status, 'FAIL');
});

test('HA preflight requires a pinned candidate, explicit mode and evidence adapter', async () => {
  for (const patch of [{ haCandidate: null }, { haScalingMode: undefined }, { haScalingMode: 'unknown' }]) {
    const driver = haDriver(await packet());
    const report = await runSuite({ ...haConfig(), ...patch }, driver, { scenario: 'ha' });
    assert.equal(report.status, 'BLOCKED'); assert.equal(driver.closed, true);
    assert.equal(report.scenarios.length, haScenarioNames.length);
  }
  for (const patch of [{ haEvidence: undefined }, { preflight: fakeDriver().preflight }]) {
    const driver = Object.assign(haDriver(await packet()), patch);
    assert.equal((await runSuite(haConfig(), driver, { scenario: 'ha' })).status, 'BLOCKED');
  }
});

test('real HA CLI keeps local safety, driver pinning and a nonzero qualification exit', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'cma-ha-cli-'));
  const entry = resolve(import.meta.dirname, '../run.mjs');
  try {
    const driverSource = `import { readFile, writeFile } from 'node:fs/promises';
export async function createDriver(config) {
  const packet = JSON.parse(await readFile(config.packetPath, 'utf8')); let step = 0;
  return {
    preflight: async () => ({ ...config, local: true, reviewed: true, syntheticUsers: true,
      mutationWindowGranted: true, haCandidateHash: packet.candidateHash }),
    contractCase: async (name) => packet.contracts.find((row) => row.name === name),
    replicaStep: async () => packet.scaling[step++],
    lossTrial: async ({ role, trial }) => packet.trials.filter((row) => row.role === role)[trial],
    haEvidence: async () => packet,
    close: async () => writeFile(config.closedPath, 'closed'),
  };
}`;
    const value = await packet(); value.trials[0].privateBody = 'PRIVATE_SENTINEL';
    const settings = { ...haConfig(), driverModule: join(directory, 'driver.mjs'), driverSha256: hash(driverSource),
      packetPath: join(directory, 'packet.json'), closedPath: join(directory, 'closed') };
    await writeFile(settings.driverModule, driverSource);
    await writeFile(settings.packetPath, JSON.stringify(value));
    for (const [name, patch, expected] of [['valid', {}, 'NO_GO'],
      ['remote', { endpoints: { am: 'https://example.invalid' } }, 'BLOCKED'],
      ['unpinned', { driverSha256: hash('other') }, 'BLOCKED']]) {
      const configPath = join(directory, `${name}.json`); const output = join(directory, name);
      await writeFile(configPath, JSON.stringify({ ...settings, ...patch }));
      const result = spawnSync(process.execPath, [entry, '--scenario', 'ha', '--config', configPath, '--out', output],
        { encoding: 'utf8', timeout: 30000 });
      assert.equal(result.status, 2, result.stderr);
      const bytes = await readFile(join(output, 'results.json'), 'utf8');
      const report = JSON.parse(bytes);
      assert.equal(report.selection, 'ha'); assert.equal(report.status, expected);
      assert.ok(!bytes.includes('PRIVATE_SENTINEL')); assert.ok(!bytes.includes('example.invalid'));
      if (name === 'valid') {
        assert.equal(report.ha.status, 'PASS'); assert.equal(report.performance.length, 0);
        assert.equal(await readFile(settings.closedPath, 'utf8'), 'closed');
        assert.match(await readFile(join(output, 'RESULTS.md'), 'utf8'), /HA component evidence: \*\*PASS\*\*/);
      } else assert.equal(report.scenarios.length, haScenarioNames.length);
    }
    const report = await runSuite(haConfig(), haDriver(await packet()), { scenario: 'ha', time: fakeTime() });
    report.ha.failures = [{ section: 'atlas', index: null, status: 'BLOCKED', code: 'missing' }];
    await saveReport(join(directory, 'render'), report);
    assert.match(await readFile(join(directory, 'render/RESULTS.md'), 'utf8'), /atlas \| all \| BLOCKED \| missing/);
  } finally { await rm(directory, { recursive: true, force: true }); }
});

for (const [name, earlier, later] of [['AM process', 0, 2], ['AM graceful', 1, 3],
  ['EG process', 101, 102], ['EG node', 106, 107]]) {
  test(`B1 rejects recounting the same ${name} death under a fresh trial label`, async () => {
    const value = await packet();
    for (const field of ['lostProcessHash', 'lostIncarnationHash', 'faultEvidenceHash']) {
      value.trials[later][field] = value.trials[earlier][field];
    }
    assert.notEqual(validateHaEvidence(candidate, value).status, 'PASS');
    const report = await runSuite(haConfig(), haDriver(value), { scenario: 'ha', time: fakeTime() });
    assert.equal(report.ha.status, 'FAIL');
  });
}

test('B1 rejects recounting an incarnation with a changed bundle or as a disruption', async () => {
  for (const section of ['trials', 'disruptions']) {
    const value = await packet(); const repeated = section === 'trials' ? value.trials[2] : value.disruptions[0];
    repeated.lostProcessHash = value.trials[0].lostProcessHash;
    repeated.lostIncarnationHash = value.trials[0].lostIncarnationHash;
    assert.notEqual(validateHaEvidence(candidate, value).status, 'PASS');
  }
});

test('B1 permits distinct incarnations in one evidence bundle and retained command reuse', async () => {
  const value = await packet();
  for (const row of [...value.trials, ...value.disruptions]) row.faultEvidenceHash = hash('shared-fault-bundle');
  assert.equal(validateHaEvidence(candidate, value).status, 'PASS');
});

for (const [index, boundary] of [[1, 'startedAtMs'], [1, 'lossStartedAtMs'],
  [101, 'startedAtMs'], [101, 'lossStartedAtMs']]) {
  test(`B1 rejects overlapping serial ${boundary} windows at trial ${index}`, async () => {
    const value = await packet(); const row = value.trials[index]; const previous = value.trials[index - 1];
    const shift = row[boundary] - previous.sustainedSuccessAtMs + 1;
    for (const field of ['startedAtMs', 'finishedAtMs', 'lossStartedAtMs', 'sustainedSuccessAtMs',
      'faultStartedAtMs', 'faultObservedAtMs', 'drainStartedAtMs']) row[field] -= shift;
    assert.notEqual(validateHaEvidence(candidate, value).status, 'PASS');
  });
}

test('B1 binds each observation to the serial requested role, mode and index', async () => {
  for (const request of [undefined, { role: 'eg', mode: 'graceful', trial: 1 },
    { role: 'am', mode: 'process', trial: 1 }, { role: 'am', mode: 'graceful', trial: 0 }]) {
    const value = await packet(); value.trials[1].request = request;
    assert.notEqual(validateHaEvidence(candidate, value).status, 'PASS');
  }
});

test('B2 requires the Web artifact in expected and observed candidate identities', async () => {
  for (const webImageDigest of [undefined, 'web:latest', `sha256:${hash('other-web')}`]) {
    const value = await packet(); value.candidate.webImageDigest = webImageDigest;
    assert.notEqual(validateHaEvidence(candidate, value).status, 'PASS');
    const expected = { ...candidate, webImageDigest };
    assert.notEqual(validateHaEvidence(expected, await packet()).status, 'PASS');
  }
});

test('B2 rejects a compatible repair bound to another Web artifact', async () => {
  const value = await packet();
  value.repairs[0].candidateHash = haCandidateHash({ ...candidate, webImageDigest: `sha256:${hash('other-web')}` });
  assert.notEqual(validateHaEvidence(candidate, value).status, 'PASS');
});

test('B2 refuses missing or changed Web artifacts during runner preflight', async () => {
  for (const webImageDigest of [undefined, `sha256:${hash('other-web')}`]) {
    const settings = haConfig(); settings.haCandidate.webImageDigest = webImageDigest;
    const driver = haDriver(await packet());
    const report = await runSuite(settings, driver, { scenario: 'ha', time: fakeTime() });
    assert.equal(report.status, 'BLOCKED'); assert.equal(driver.closed, true);
    assert.ok(report.scenarios.every((row) => row.status === 'BLOCKED'));
  }
});
