import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { operationLimitsMs, qualifyOperations, readerDisposition, previewFleetPatch }
  from './agent-log-activation-evidence.mjs';

const minute = 60_000;
const deadline = Date.parse('2026-11-14T14:00:00+08:00');

function sampleWindow(id, fleet_percent, started_ms, ended_ms) {
  return {
    fleet_percent, started_ms, ended_ms,
    binding: {
      source: 'fixture_source', image: 'fixture_image', generation: id,
      configuration: id, cohort: 'fixture_cohort', window: id, replicas: 2,
      workers: 'fixture_workers', mongo_pool: 'fixture_pool', resource_limits: 'fixture_limits',
    },
    operations: Object.fromEntries(Object.entries(operationLimitsMs).map(([name, limit]) => [name, [{
      client_send_ms: started_ms + 100, completed_ms: started_ms + 100 + limit,
      ack_ms: started_ms + 101,
      outcome: 'success', first_use: true, boundary: 'whole_operation',
    }]])),
  };
}

function evidence() {
  return {
    baseline: sampleWindow('baseline', 0, deadline - 91 * minute, deadline - 61 * minute),
    watch: sampleWindow('watch', 10, deadline - 60 * minute, deadline),
  };
}

test('only fully attributed complete-operation evidence qualifies, without mutation', () => {
  const packet = evidence();
  const original = structuredClone(packet);
  assert.equal(qualifyOperations(packet, deadline).status, 'QUALIFIED');
  assert.deepEqual(packet, original);
  for (const phase of ['baseline', 'watch']) {
    for (const field of Object.keys(packet[phase].binding)) {
      const missing = evidence();
      delete missing[phase].binding[field];
      assert.equal(qualifyOperations(missing, deadline).status, 'HOLD', `${phase}: ${field}`);
    }
  }
  assert.equal(qualifyOperations(null).status, 'HOLD');
});

test('each hard limit, missing path, first-use sample and completion boundary refuses', () => {
  for (const name of Object.keys(operationLimitsMs)) {
    for (const fault of ['over', 'missing', 'ack_only', 'incomplete', 'first_use', 'negative', 'nan']) {
      const packet = evidence();
      const sample = packet.watch.operations[name][0];
      if (fault === 'over') sample.completed_ms++;
      if (fault === 'missing') packet.watch.operations[name] = [];
      if (fault === 'ack_only') sample.boundary = 'admission_ack';
      if (fault === 'incomplete') delete sample.completed_ms;
      if (fault === 'first_use') sample.first_use = false;
      if (fault === 'negative') sample.completed_ms = 0;
      if (fault === 'nan') sample.completed_ms = NaN;
      assert.equal(qualifyOperations(packet, deadline).status, 'HOLD', `${name}: ${fault}`);
    }
  }
});

test('early ACK and favorable p95 cannot hide late completion or any exceedance', () => {
  const packet = evidence();
  const sample = packet.watch.operations.send_complete[0];
  packet.watch.operations.send_complete = Array.from({ length: 100 }, () =>
    ({ ...sample, completed_ms: sample.client_send_ms + 100 }));
  packet.watch.operations.send_complete.push({ ...sample, completed_ms: sample.client_send_ms + 1001 });
  packet.p95_ms = 100;
  const result = qualifyOperations(packet, deadline);
  assert.equal(result.status, 'HOLD');
  assert.equal(result.operations.send_complete.maximum_ms, 1001);
  assert.deepEqual(result.operations.send_complete.exceedances, [{ index: 100, elapsed_ms: 1001 }]);
});

test('error and cancellation counts stay in the denominator and hold acceptance', () => {
  const packet = evidence();
  const samples = packet.watch.operations.tab_open;
  samples.push({ ...samples[0], outcome: 'error' }, { ...samples[0], outcome: 'cancelled' });
  const result = qualifyOperations(packet, deadline);
  assert.equal(result.status, 'HOLD');
  assert.equal(result.operations.tab_open.samples, 3);
  for (const key of ['success', 'error', 'cancelled']) assert.equal(result.operations.tab_open[key], 1);
});

test('matches the baseline settings and enforces both window bounds and the deadline', () => {
  for (const key of ['image', 'cohort', 'replicas', 'workers', 'mongo_pool', 'resource_limits']) {
    const packet = evidence();
    packet.watch.binding[key] = key === 'replicas' ? 1 : 'different';
    assert.ok(qualifyOperations(packet, deadline).reasons.includes('unmatched_baseline'), key);
  }
  const cases = [
    ['incomplete_off_baseline', packet => packet.baseline.started_ms++],
    ['incomplete_off_baseline', packet => packet.baseline.fleet_percent = 10],
    ['incomplete_stage_watch', packet => packet.watch.started_ms++],
    ['incomplete_stage_watch', packet => packet.watch.fleet_percent = 0],
    ['overlapping_evidence', packet => packet.baseline.ended_ms = packet.watch.started_ms + 1],
    ['overlapping_evidence', packet => packet.watch.binding.window = packet.baseline.binding.window],
    ['deadline_not_met', packet => packet.watch.ended_ms++],
  ];
  for (const [reason, change] of cases) {
    const packet = evidence();
    change(packet);
    assert.ok(qualifyOperations(packet, deadline).reasons.includes(reason), reason);
  }
  assert.equal(qualifyOperations(evidence()).status, 'QUALIFIED');
  assert.ok(qualifyOperations(evidence(), deadline - 1).reasons.includes('deadline_not_met'));
  assert.ok(qualifyOperations(evidence(), NaN).reasons.includes('deadline_not_met'));
});

test('each operation belongs to its own phase including completion at the right boundary', () => {
  const packet = evidence();
  packet.watch.operations.tab_open = packet.baseline.operations.tab_open;
  assert.ok(qualifyOperations(packet).reasons.includes('tab_open:outside_phase'));

  const good = evidence();
  const sample = good.watch.operations.send_complete[0];
  sample.client_send_ms = good.watch.ended_ms - 500;
  sample.completed_ms = good.watch.ended_ms;
  assert.equal(qualifyOperations(good).status, 'QUALIFIED');
  sample.completed_ms++;
  assert.ok(qualifyOperations(good).reasons.includes('send_complete:outside_phase'));

  sample.client_send_ms = good.watch.ended_ms;
  sample.completed_ms = good.watch.ended_ms;
  assert.ok(qualifyOperations(good).reasons.includes('send_complete:outside_phase'));
});

test('a caller deadline after the campaign cutoff cannot admit late evidence', () => {
  const tomorrow = 24 * 60 * minute;
  const packet = {
    baseline: sampleWindow('baseline', 0, deadline + tomorrow - 91 * minute,
      deadline + tomorrow - 61 * minute),
    watch: sampleWindow('watch', 10, deadline + tomorrow - 60 * minute, deadline + tomorrow),
  };
  const result = qualifyOperations(packet, deadline + tomorrow);
  assert.deepEqual(result.reasons, ['deadline_not_met']);
  assert.equal(result.status, 'HOLD');
  assert.equal(result.deadline_ms, deadline);
});

test('a send that starts during the watch retains its timing when it finishes after the watch', () => {
  const packet = evidence();
  const sends = packet.watch.operations.send_complete;
  sends[0].completed_ms = sends[0].client_send_ms + 100;
  sends.push({
    ...sends[0],
    client_send_ms: packet.watch.ended_ms - 1000,
    completed_ms: packet.watch.ended_ms + 1,
  });
  const result = qualifyOperations(packet);
  assert.equal(result.status, 'HOLD');
  assert.ok(result.reasons.includes('send_complete:outside_phase'));
  const summary = result.operations.send_complete;
  assert.equal(summary.success, 2);
  assert.equal(summary.maximum_ms, 1001);
  assert.deepEqual(summary.elapsed_ms, [
    { index: 0, elapsed_ms: 100 }, { index: 1, elapsed_ms: 1001 },
  ]);
  assert.deepEqual(summary.exceedances, [{ index: 1, elapsed_ms: 1001 }]);
});

test('retains measured baseline timings and errors for comparison', () => {
  const packet = evidence();
  const samples = packet.baseline.operations.interrupt_complete;
  samples[0].completed_ms++;
  samples.push({ ...samples[0], completed_ms: samples[0].client_send_ms + 200, outcome: 'error' });
  const result = qualifyOperations(packet, deadline);
  assert.equal(result.status, 'QUALIFIED');
  assert.equal(result.baseline.interrupt_complete.error, 1);
  assert.deepEqual(result.baseline.interrupt_complete.elapsed_ms, [
    { index: 0, elapsed_ms: 1001 }, { index: 1, elapsed_ms: 200 },
  ]);
  assert.deepEqual(result.baseline.interrupt_complete.exceedances, [{ index: 0, elapsed_ms: 1001 }]);
});

test('both readers HOLD for absent scope or approval and retain the future scope HOLD', () => {
  for (const stage of ['title', 'approval']) {
    for (const scope of [undefined, 'fixture_scope']) {
      for (const approval of [undefined, 'fixture_approval']) {
        const packet = { reviewed_scope_binding: scope, individual_owner_approval: approval,
          fleet_percent: 100, stable_seconds: 3600, mandatory_v2_reader_floor: true,
          flags: { title: false, approval: false }, cohort: ['agt_owner_test'] };
        const original = structuredClone(packet);
        const result = readerDisposition(stage, packet);
        assert.equal(result.status, 'HOLD');
        assert.equal(result.patch, null);
        if (!scope) assert.ok(result.reasons.includes('reviewed_scope_binding_required'));
        if (!approval) assert.ok(result.reasons.includes('individual_owner_approval_required'));
        assert.ok(result.reasons.includes('legacy_v2_scope_fence_deferred'));
        assert.deepEqual(packet, original);
      }
    }
    const result = readerDisposition(stage, {});
    assert.ok(result.reasons.includes('fleet_100_plus_stable_hour_required'));
    assert.ok(result.reasons.includes('mandatory_v2_reader_floor_required'));
  }
});

test('stage previews refuse failed or unknown preconditions with zero mutation', () => {
  for (const stage of ['off', '10', '50', '100']) {
    const patch = JSON.parse(readFileSync(new URL(`../agent-log-stages/${stage}.json`, import.meta.url)));
    for (const adoption of ['false', 'true', undefined]) {
      for (const recovery of ['false', 'true', undefined]) {
        const before = { CHRONO_AGENT_LOG_SEGMENT_ADOPTION_ENABLED: adoption,
          CHRONO_AGENT_CAPACITY_RECOVERY_ENABLED: recovery, unrelated: 'retained',
          CHRONO_AGENT_LOG_ADMISSION_AGENTS: 'agt_old', CHRONO_AGENT_LOG_FLEET_PERCENT: '50' };
        const original = structuredClone({ before, patch });
        const result = previewFleetPatch(before, patch);
        assert.deepEqual({ before, patch }, original);
        if (stage !== 'off' && (adoption !== 'false' || recovery !== 'false')) {
          assert.equal(result.status, 'HOLD');
          assert.equal(result.patch, null);
          assert.equal(result.after, undefined);
        } else {
          assert.equal(result.status, 'PREVIEW_ONLY');
          assert.equal(result.after.CHRONO_AGENT_LOG_SEGMENT_ADOPTION_ENABLED, adoption);
          assert.equal(result.after.CHRONO_AGENT_CAPACITY_RECOVERY_ENABLED, recovery);
          assert.equal(result.after.unrelated, 'retained');
          assert.equal(result.after.CHRONO_AGENT_LOG_ADMISSION_AGENTS, '');
          assert.equal(result.after.CHRONO_AGENT_LOG_FLEET_PERCENT, stage === 'off' ? '0' : stage);
        }
      }
    }
  }
  assert.equal(previewFleetPatch({}, { data: { unexpected: 'true' } }).status, 'HOLD');
  const stage = JSON.parse(readFileSync(new URL('../agent-log-stages/10.json', import.meta.url)));
  const off = JSON.parse(readFileSync(new URL('../agent-log-stages/off.json', import.meta.url)));
  assert.equal(previewFleetPatch(undefined, stage).status, 'HOLD');
  assert.equal(previewFleetPatch(undefined, off).status, 'PREVIEW_ONLY');
  const inherited = Object.create({
    CHRONO_AGENT_LOG_SEGMENT_ADOPTION_ENABLED: 'false',
    CHRONO_AGENT_CAPACITY_RECOVERY_ENABLED: 'false',
  });
  assert.equal(previewFleetPatch(inherited, stage).status, 'HOLD');
});
