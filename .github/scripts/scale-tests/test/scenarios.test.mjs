import test from 'node:test';
import assert from 'node:assert/strict';
import { crossReplica, suddenLoss } from '../scenarios/correctness.mjs';
import { autoscale, checkSnapshot } from '../scenarios/scaling.mjs';
import { runSuite } from '../lib/suite.mjs';
import { scenarioNames } from '../lib/report.mjs';
import { hash } from '../lib/checks.mjs';
import { config, fakeDriver, fakeTime, proof, snapshot } from './fixtures.mjs';

test('all ordered create/use routes exercise every client path with cleanup', async () => {
  const driver = fakeDriver();
  assert.deepEqual(await crossReplica(driver), { journeys: 9, paths: 6, replicas: 2 });
  assert.equal(driver.cleaned, 9);
  driver.action = async () => ({ ...proof(), ok: false });
  await assert.rejects(crossReplica(driver), /client_path_failed/);
  assert.equal(driver.cleaned, 10);
});

test('cross-replica gates fail on topology, serving identity, desktop and stream gaps', async () => {
  for (const patch of [
    { routes: async () => [] },
    { routes: async () => [{ kind: 'replica' }, { kind: 'replica' }, { kind: 'balancer' }] },
    { action: async () => ({ ...proof(), frameReceived: false }) },
    { confirmStream: async () => ({ ...proof(), durableContinuity: false }) },
  ]) await assert.rejects(crossReplica(Object.assign(fakeDriver(), patch)));
});

test('fault recovery counts injection time and requires just-renewed authority plus actual death', async () => {
  for (const mode of ['am_stream', 'am_command', 'eg_inbound']) {
    const driver = fakeDriver();
    assert.equal((await suddenLoss(driver, mode, config, fakeTime())).commands, 1);
    assert.equal(driver.cleaned, 1);
  }
  const driver = fakeDriver(); const time = fakeTime();
  const original = driver.killOwner;
  driver.killOwner = async (work) => { await time.sleep(30001); return original(work); };
  await assert.rejects(suddenLoss(driver, 'am_command', config, time), /deadline/);
  driver.killOwner = async () => ({ abrupt: true, processDeathObserved: false });
  await assert.rejects(suddenLoss(driver, 'am_stream', config, fakeTime()), /death_unproven/);
  const start = driver.beginInFlight;
  driver.beginInFlight = async () => ({ ...await start(), leaseJustRenewed: false });
  await assert.rejects(suddenLoss(driver, 'am_command', config, fakeTime()), /worst_case/);
  await assert.rejects(suddenLoss(driver, 'unknown', config, fakeTime()));
  const unrelated = fakeDriver(); const begin = unrelated.beginInFlight;
  unrelated.beginInFlight = async () => ({ ...await begin(), commandHash: hash('unrelated-accepted-command') });
  await assert.rejects(suddenLoss(unrelated, 'am_command', config, fakeTime()), /original_fault_command_missing/);
});

test('a hung injection or recovery read expires and aborts the driver before cleanup', async () => {
  for (const operation of ['killOwner', 'observeRecovery']) {
    const driver = fakeDriver();
    let signal;
    let reconciled = false;
    driver[operation] = async (_work, options) => {
      signal = options.signal;
      return new Promise(() => {});
    };
    driver.cleanup = async () => {
      assert.equal(signal?.aborted, true);
      assert.equal(signal.reason.code, 'recovery_deadline_exceeded');
      driver.cleaned++;
    };
    driver.reconcile = async () => { reconciled = true; };
    await assert.rejects(suddenLoss(driver, 'am_command', { ...config, recoveryMs: 25 }, fakeTime()),
      { code: 'recovery_deadline_exceeded' });
    assert.equal(reconciled, false);
    assert.equal(driver.cleaned, 1);
  }
});

test('autoscale preserves canaries during stress removal and requires workload evidence', async () => {
  for (const users of [2, 100]) {
    const driver = fakeDriver();
    const result = await autoscale(driver, config, { users, time: fakeTime() });
    assert.equal(result.peak_am, 4); assert.equal(result.peak_eg, 4);
    assert.equal(result.sequence, '2-4-2');
    assert.equal(driver.cleaned, 1); assert.equal(driver.stopped, 1);
  }
  const driver = fakeDriver(); driver.finishUsers = async () => ({ ...snapshot(), actualReadsPerSecond: 9 });
  await assert.rejects(autoscale(driver, config, { users: 100, time: fakeTime() }), /workload/);
  driver.snapshot = async () => snapshot(2);
  await assert.rejects(autoscale(driver, config, { time: fakeTime() }), /deadline/);
  assert.equal(driver.stopped, 2);
  for (const patch of [{ failedRequests: 1 }, { droppedRequests: 1 }, { streamGaps: 1 },
    { staleWrites: 1 }, { connectedUsers: 99 }, { openStreams: 99 }, { evidenceHash: null }]) {
    assert.throws(() => checkSnapshot({ ...snapshot(), ...patch }, 100));
  }
  assert.throws(() => checkSnapshot({ ...snapshot(), am: { ...snapshot().am, metricsAvailable: false } }, 100));
});

test('a hung autoscale observation aborts before stress and cohort cleanup', async () => {
  for (const stalledSample of [1, 2, 3]) {
    const driver = fakeDriver();
    const original = driver.snapshot;
    let sampled = 0;
    let signal;
    driver.snapshot = async (cohort, options) => {
      if (++sampled !== stalledSample) return original(cohort, options);
      signal = options.signal;
      return new Promise(() => {});
    };
    driver.cleanup = async () => {
      assert.equal(signal?.aborted, true);
      assert.equal(signal.reason.code, 'scaling_observation_deadline_exceeded');
      driver.cleaned++;
    };
    await assert.rejects(autoscale(driver, { ...config, scaleTimeoutMs: 25 }, { time: fakeTime() }),
      { code: 'scaling_observation_deadline_exceeded' });
    assert.equal(driver.cleaned, 1);
    assert.equal(driver.stopped, stalledSample === 1 ? 0 : 1);
  }
});

test('suite reports failures, aborts later gates and never turns missing fixtures into a GO', async () => {
  const driver = fakeDriver(); const progress = [];
  const report = await runSuite(config, driver, { time: fakeTime(), checkpoint: async () => {}, samples: async () => ({}), progress: (line) => progress.push(line) });
  assert.equal(report.status, 'NO_GO'); assert.equal(report.track_status, 'INCOMPLETE');
  assert.ok(report.scenarios.slice(0, -1).every((row) => row.status === 'PASS'));
  assert.equal(progress.length, scenarioNames.length); assert.equal(driver.closed, true);
  const missing = await runSuite(config, undefined);
  assert.equal(missing.status, 'BLOCKED'); assert.equal(missing.scenarios.length, scenarioNames.length);
  const broken = fakeDriver(); broken.action = async () => { throw new Error('raw response'); };
  const failure = await runSuite(config, broken, { time: fakeTime() });
  assert.equal(failure.status, 'NO_GO'); assert.equal(failure.scenarios[0].status, 'FAIL');
  assert.ok(failure.scenarios.slice(1).every((s) => s.status === 'BLOCKED'));
  assert.ok(!JSON.stringify(failure).includes('raw response')); assert.equal(broken.closed, true);
  const changed = fakeDriver(); changed.preflight = async () => ({});
  assert.equal((await runSuite(config, changed)).status, 'BLOCKED');
  assert.equal(changed.closed, true, 'preflight failures must release constructed drivers');
  const preflightLeak = fakeDriver(); preflightLeak.preflight = async () => ({});
  preflightLeak.close = async () => { throw new Error('private cleanup details'); };
  const preflightReport = await runSuite(config, preflightLeak);
  assert.equal(preflightReport.cleanup.completed, false);
  assert.ok(preflightReport.scenarios.some((row) => row.scenario === 'cleanup' && row.status === 'FAIL'));
  const leak = fakeDriver(); leak.close = async () => { throw new Error('cleanup'); };
  assert.ok((await runSuite(config, leak, { time: fakeTime() })).scenarios.some((row) => row.scenario === 'cleanup'));
});

test('cohort source identity is bound to environment preflight before any N2 measurement', async () => {
  const driver = fakeDriver(); const measure = driver.measureCohort; const calls = [];
  driver.measureCohort = async (phase) => {
    calls.push(phase);
    return { ...await measure(phase), sourceHash: hash('different-build') };
  };
  const report = await runSuite(config, driver, { time: fakeTime(), checkpoint: async () => {}, samples: async () => ({}) });
  assert.deepEqual(calls, ['n1']);
  assert.equal(report.scenarios.find((row) => row.scenario === 'performance').code, 'measurement_provenance_changed');
  assert.equal(driver.closed, true);
});
