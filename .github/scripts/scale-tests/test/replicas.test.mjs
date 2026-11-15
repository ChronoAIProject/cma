import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { join } from 'node:path';
import { measurePerformance } from '../lib/measurement.mjs';
import { saveSamples } from '../lib/report.mjs';
import { compareCohorts } from '../lib/metrics.mjs';
import { validateConfig } from '../lib/safety.mjs';
import { autoscale } from '../scenarios/scaling.mjs';
import { cohort, config, fakeDriver, fakeTime, snapshot } from './fixtures.mjs';

test('M3 four-replica and historical five-replica samples retain their actual sequence', async () => {
  const directory = await mkdtemp(join(import.meta.dirname, '.replicas-'));
  try {
    for (const peak of [4, 5]) {
      const report = {};
      const path = join(directory, String(peak));
      await measurePerformance(fakeDriver(peak), { ...config, scalingPeak: peak }, report,
        { checkpoint: async () => {}, samples: (value, index) => saveSamples(path, value, index) });
      const raw = JSON.parse(await readFile(join(path, 'cohort-4-scaling.json'), 'utf8'));
      assert.equal(raw.observedReplicas, `2-${peak}-2`);
      assert.ok(report.performance.filter((row) => row.phase === 'scaling')
        .every((row) => row.observedReplicas === raw.observedReplicas && row.status === 'PASS'));
    }
  } finally { await rm(directory, { recursive: true }); }
});

test('a five-replica result cannot satisfy a registered four-replica comparison', async () => {
  const values = ['n1', 'n1', 'n1', 'n2', 'scaling'].map((phase) => cohort(phase));
  values[4].observedReplicas = '2-5-2';
  assert.throws(() => compareCohorts(values, config.requiredRoutes), /replica_evidence/);
  await assert.rejects(measurePerformance(fakeDriver(5), config, {},
    { checkpoint: async () => {}, samples: async () => ({}) }), /replica_evidence/);
  for (const invalid of [3, 6, '4', null]) {
    assert.throws(() => validateConfig({ ...config, scalingPeak: invalid }), /invalid_scaling_peak/);
  }
});

test('autoscale uses the declared peak and still fails on a downscale canary error', async () => {
  const historical = await autoscale(fakeDriver(5), { ...config, scalingPeak: 5 }, { time: fakeTime() });
  assert.equal(historical.sequence, '2-5-2'); assert.equal(historical.peak_am, 5);
  await assert.rejects(autoscale(fakeDriver(5), config, { time: fakeTime() }), /scaling_peak_exceeded/);
  const driver = fakeDriver(); let reads = 0;
  driver.startStress = async (_cohort, options) => {
    assert.equal(options.targetReplicas, 4);
    return {};
  };
  driver.snapshot = async () => {
    reads++;
    return { ...snapshot(reads === 2 ? 4 : 2), failedRequests: reads === 3 ? 1 : 0 };
  };
  await assert.rejects(autoscale(driver, config, { time: fakeTime() }), /user_request_failed/);
  assert.equal(driver.stopped, 1); assert.equal(driver.cleaned, 1);
});
