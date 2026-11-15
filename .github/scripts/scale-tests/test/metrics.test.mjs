import test from 'node:test';
import assert from 'node:assert/strict';
import { compareCohorts, summarize } from '../lib/metrics.mjs';
import { startLoad } from '../lib/load.mjs';
import { config, cohort, fakeTime } from './fixtures.mjs';

test('nearest-rank percentiles retain failures and suppress undersampled p99', () => {
  assert.equal(summarize([]).p95_ms, null);
  const values = Array.from({ length: 1000 }, (_, i) => ({ ok: true, elapsedMs: i + 1 }));
  assert.equal(summarize(values).p50_ms, 500);
  assert.equal(summarize(values).p95_ms, 950);
  assert.equal(summarize(values).p99_ms, 990);
  assert.equal(summarize(values.slice(1)).p99_ms, null);
  values[0].ok = false;
  assert.equal(summarize(values).failed, 1);
  assert.throws(() => summarize([{ ok: true, elapsedMs: -1 }]));
});

test('matched cohorts reject regressions, noise, failures, missing metrics and mismatched provenance', () => {
  const cohorts = [cohort('n1'), cohort('n1'), cohort('n2'), cohort('scaling'), cohort('n1')];
  assert.ok(compareCohorts(cohorts, config.requiredRoutes).every((r) => r.status === 'PASS'));
  cohorts[2] = cohort('n2', 110);
  assert.ok(compareCohorts(cohorts, config.requiredRoutes).some((r) => r.status === 'FAIL'));
  cohorts[1] = cohort('n1', 130);
  assert.ok(compareCohorts(cohorts, config.requiredRoutes).every((r) => r.status === 'BLOCKED'));
  cohorts[1] = cohort('n1'); cohorts[2] = cohort('n2');
  cohorts[2].samples.send[0].ok = false;
  assert.equal(compareCohorts(cohorts, config.requiredRoutes).find((r) => r.metric === 'send' && r.phase === 'n2').failed, 1);
  cohorts[2].hardwareHash = 'bad';
  assert.throws(() => compareCohorts(cohorts, config.requiredRoutes), /provenance/);
  cohorts[2] = cohort('n2'); cohorts[2].samples.send[0].rendered = false;
  assert.throws(() => compareCohorts(cohorts, config.requiredRoutes), /rendered/);
  assert.throws(() => compareCohorts(cohorts.slice(1), config.requiredRoutes));
  assert.throws(() => compareCohorts(cohorts, []));
  assert.throws(() => compareCohorts(cohorts, config.requiredRoutes, 5));
});

test('open-loop workload includes failures and generator queueing without coordinated omission', async () => {
  const time = fakeTime(); let reads = 0; let turns = 0;
  const run = startLoad({ read: async () => { reads++; }, turn: async () => { turns++; }, durationMs: 6000, time });
  const samples = await run.done;
  assert.equal(reads, 60); assert.equal(turns, 2); assert.equal(samples.length, 62);
  assert.ok(samples.every((s) => s.ok));
  const failed = startLoad({ read: async () => { throw new Error('network'); }, turn: async () => {}, durationMs: 100, time });
  assert.equal((await failed.done).filter((s) => !s.ok).length, 1);
  assert.throws(() => startLoad({ durationMs: 0 }));
  const stop = startLoad({ read: async () => {}, turn: async () => {}, durationMs: 1000, time });
  assert.ok((await stop.stop()).length <= 2);
});
