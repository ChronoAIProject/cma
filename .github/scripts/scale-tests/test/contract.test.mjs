import test from 'node:test';
import assert from 'node:assert/strict';
import { contractCampaign, trialCampaign, partitionLoad, replicaSweep } from '../scenarios/contract.mjs';
import { deltaInterval } from '../lib/confidence.mjs';
import { fakeDriver, contractProof } from './fixtures.mjs';

test('contract campaigns execute all production-path requirements and reject missing adapters', async () => {
  const driver = fakeDriver();
  assert.equal((await contractCampaign(driver)).cases, 23);
  assert.equal((await replicaSweep(driver)).steps.length, 4);
  assert.equal((await partitionLoad(driver)).proofs.length, 2);
  for (const action of [contractCampaign, replicaSweep, partitionLoad]) await assert.rejects(action({}));
  driver.contractCase = async () => ({ ...contractProof(), clientHashes: [] });
  await assert.rejects(contractCampaign(driver), /independent/);
  driver.contractCase = async () => ({ ...contractProof(), passed: false });
  await assert.rejects(contractCampaign(driver), /contract_failed/);
  const step = fakeDriver().replicaStep;
  driver.replicaStep = async (request) => {
    const value = await step(request);
    return { ...value, am: { ...value.am, ready: 1 } };
  };
  await assert.rejects(replicaSweep(driver), /sweep_failed/);
  const load = fakeDriver().partitionLoad;
  driver.partitionLoad = async () => ({ ...await load(), unknown: 1 });
  await assert.rejects(partitionLoad(driver), /delivery_unproven/);
});

test('trial campaign requires101 AM and5 process+5 node EG trials, unique evidence and bounded service recovery', async () => {
  const driver = fakeDriver();
  const am = await trialCampaign(driver, 'am');
  const eg = await trialCampaign(driver, 'eg');
  assert.equal(am.trials.length, 101); assert.equal(eg.trials.length, 10);
  assert.equal(eg.trials.filter((t) => t.mode === 'node').length, 5);
  await assert.rejects(trialCampaign({}, 'am'));
  const original = driver.lossTrial;
  driver.lossTrial = async (request) => ({ ...await original(request), nodeLossObserved: false });
  await assert.rejects(trialCampaign(driver, 'eg'), /node_loss/);
  driver.lossTrial = async (request) => ({ ...await original(request), recoveryMs: 30001 });
  await assert.rejects(trialCampaign(driver, 'am'), /recovery_failed/);
});

test('confidence intervals are repeatable and detect a fixed p95 shift', () => {
  const baseline = Array(100).fill(100); const candidate = Array(100).fill(110);
  const value = deltaInterval(baseline, candidate);
  assert.equal(value.lower_ms, 10); assert.equal(value.upper_ms, 10);
  assert.deepEqual(value, deltaInterval(baseline, candidate));
  assert.throws(() => deltaInterval([1], [1]));
  assert.throws(() => deltaInterval(baseline, candidate, { draws: 10 }));
});

test('HA plans observe AM 2-3-5-2 and EG 2-3 with independent envelopes', async () => {
  for (const mode of ['manual', 'hpa']) {
    const driver = fakeDriver(); const step = driver.replicaStep; const requests = [];
    driver.replicaStep = async (request) => {
      requests.push(request);
      const value = await step(request);
      if (mode === 'manual') {
        for (const role of ['am', 'eg']) Object.assign(value[role], { hpa: false, metricsAvailable: false });
      }
      return value;
    };
    const result = await replicaSweep(driver, { mode });
    assert.deepEqual(result.plans, { am: [2, 3, 5, 2], eg: [2, 3] });
    assert.deepEqual(requests.map(({ am, eg }) => [am, eg]), [[2, 2], [3, 3], [5, 3], [2, 3]]);
    assert.deepEqual(result.steps.map((step) => step.observed_ready), requests.map(({ am, eg }) => ({ am, eg })));
    assert.ok(requests.every((r) => r.mode === mode && r.users === 100 && r.streams === 100
      && r.readsPerSecond === 10 && r.turnsPerMinute === 20));
  }
  await assert.rejects(replicaSweep(fakeDriver(), { mode: 'unknown' }), /mode_invalid/);
});

test('HA scaling refuses configured-only counts, absent traffic, bad recovery and missing capacity', async () => {
  for (const mutate of [
    (v) => { v.am.ready = 1; },
    (v) => { v.eg.desired = 5; },
    (v) => { v.am.servingReplicaHashes = []; },
    (v) => { v.am.servingReplicaHashes.fill(v.am.servingReplicaHashes[0]); },
    (v) => { v.minimumConnectedUsers = 99; },
    (v) => { v.minimumConnectedUsers = '100'; },
    (v) => { v.minimumOpenStreams = Infinity; },
    (v) => { delete v.minimumOpenStreams; },
    (v) => { delete v.actualReadsPerSecond; },
    (v) => { v.actualTurnsPerMinute = 19; },
    (v) => { v.attemptedRequests = 0; },
    (v) => { v.wrongRoutes = 1; },
    (v) => { v.recoveryMs = 30001; },
    (v) => { v.sustainedSuccess = false; },
    (v) => { v.am.pdb = false; },
    (v) => { v.mongoHeadroom = 0.29; },
    (v) => { v.allClientsAccounted = false; },
    (v) => { v.terminatingProcessesWithinBound = false; },
    (v) => { v.oracle.commands[0].effects = 2; },
    (v) => { v.scalingMode = 'hpa'; },
  ]) {
    const driver = fakeDriver(); const step = driver.replicaStep;
    driver.replicaStep = async (request) => { const value = await step(request); mutate(value); return value; };
    await assert.rejects(replicaSweep(driver));
  }
});

test('HPA HA scaling requires working metrics and observed CPU or connection triggers', async () => {
  for (const patch of [{ hpa: false }, { metricsAvailable: false }, { min: 1 }, { max: 4 },
    { hpaEventsObserved: false }, { trigger: 'unknown' }]) {
    const driver = fakeDriver(); const step = driver.replicaStep;
    driver.replicaStep = async (request) => {
      const value = await step(request); Object.assign(value.am, patch); return value;
    };
    await assert.rejects(replicaSweep(driver, { mode: 'hpa' }), /hpa_or_disruption/);
  }
});
