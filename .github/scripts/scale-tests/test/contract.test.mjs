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
  driver.replicaStep = async () => ({ ...contractProof(), am: 1 });
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
