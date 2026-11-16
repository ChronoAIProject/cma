import test from 'node:test';
import assert from 'node:assert/strict';
import { runSuite } from '../lib/suite.mjs';
import { config, fakeDriver, cohort } from './fixtures.mjs';

function baselineDriver(transform = (value) => value) {
  const calls = [];
  const driver = {
    preflight: fakeDriver().preflight,
    async measureCohort(phase) { calls.push(phase); return transform(cohort(phase)); },
    async close() { calls.push('close'); },
  };
  return { driver, calls };
}
const options = { scenario: 'baseline-n1', checkpoint: async () => {}, samples: async () => ({}) };

test('baseline selection captures three N1 windows without fault, scale or cross-replica calls', async () => {
  const { driver, calls } = baselineDriver();
  const retained = [];
  const report = await runSuite(config, driver, { ...options,
    samples: async (value, index) => { retained.push([value.phase, index]); return { index }; } });
  assert.deepEqual(calls, ['n1', 'n1', 'n1', 'close']);
  assert.deepEqual(retained, [['n1', 0], ['n1', 1], ['n1', 2]]);
  assert.equal(report.selection, 'baseline-n1');
  assert.equal(report.scenarios[0].scenario, 'n1_capture');
  assert.equal(report.scenarios[0].status, 'PASS');
  assert.equal(report.scenarios[0].evidence.comparison_qualified, false);
  assert.equal(report.performance.length, 43 * 3);
  assert.ok(report.performance.every((row) => row.status === 'BLOCKED' && row.delta_ms === null));
  assert.equal(report.status, 'NO_GO');
});

test('baseline capture preserves missing routes, failed attempts and unrendered or slow journeys', async () => {
  const cases = [
    [(value) => { delete value.samples['scorecard-L32']; }, 'baseline_samples_incomplete', 'BLOCKED'],
    [(value) => { value.samples.send[0].ok = false; }, 'baseline_request_failed', 'FAIL'],
    [(value) => { value.samples.open[0].rendered = false; }, 'rendered_completion_not_proven', 'BLOCKED'],
    [(value) => { value.samples.create[0].elapsedMs = 5001; }, 'baseline_latency_limit_exceeded', 'FAIL'],
  ];
  for (const [mutate, code, status] of cases) {
    const { driver, calls } = baselineDriver((value) => { mutate(value); return value; });
    let saved = 0; let checkpointed = false;
    const report = await runSuite(config, driver, { ...options,
      checkpoint: async () => { checkpointed = true; }, samples: async () => ({ index: saved++ }) });
    assert.equal(saved, 3); assert.equal(checkpointed, true);
    assert.equal(report.scenarios[0].code, code); assert.equal(report.scenarios[0].status, status);
    assert.equal(report.performance.length, 129); assert.equal(calls.at(-1), 'close');
  }
});

test('baseline capture rejects changed phase, wrong replica count and broken evidence sinks', async () => {
  for (const [patch, code] of [
    [{ phase: 'n2' }, 'measurement_phase_changed'],
    [{ observedReplicas: 2 }, 'cohort_replica_evidence_missing'],
  ]) {
    const { driver } = baselineDriver((value) => ({ ...value, ...patch }));
    const report = await runSuite(config, driver, options);
    assert.equal(report.scenarios[0].code, code); assert.equal(report.cleanup.completed, true);
    assert.equal(report.sample_artifacts.length, 0);
  }
  const { driver, calls } = baselineDriver();
  const report = await runSuite(config, driver, { ...options,
    samples: async () => { throw new Error('private filesystem detail'); } });
  assert.deepEqual(calls, ['n1', 'close']);
  assert.equal(report.scenarios[0].status, 'FAIL');
  assert.ok(!JSON.stringify(report).includes('private filesystem detail'));
});

test('invalid selections and missing baseline drivers remain blocked', async () => {
  const { driver, calls } = baselineDriver();
  assert.equal((await runSuite(config, driver, { scenario: 'unknown' })).scenarios[0].code, 'invalid_scenario');
  assert.deepEqual(calls, ['close']);
  const missing = await runSuite(config, undefined, options);
  assert.equal(missing.scenarios.length, 1);
  assert.equal(missing.scenarios[0].scenario, 'n1_capture');
  assert.equal(missing.status, 'BLOCKED');
});
