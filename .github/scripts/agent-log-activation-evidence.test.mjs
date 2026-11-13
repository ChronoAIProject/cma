import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { operationLimitsMs, qualifyOperations, readerDisposition, previewFleetPatch }
  from './agent-log-activation-evidence.mjs';

function evidence() {
  return {
    binding: Object.fromEntries(['source', 'image', 'generation', 'configuration', 'cohort', 'window']
      .map(key => [key, `fixture_${key}`])),
    operations: Object.fromEntries(Object.entries(operationLimitsMs).map(([name, limit]) => [name, [{
      client_send_ms: 100, completed_ms: 100 + limit, ack_ms: 101,
      outcome: 'success', first_use: true, boundary: 'whole_operation',
    }]])),
  };
}

test('only fully attributed complete-operation evidence qualifies, without mutation', () => {
  const packet = evidence();
  const original = structuredClone(packet);
  assert.equal(qualifyOperations(packet).status, 'QUALIFIED');
  assert.deepEqual(packet, original);
  for (const field of Object.keys(packet.binding)) {
    const missing = evidence();
    delete missing.binding[field];
    assert.equal(qualifyOperations(missing).status, 'HOLD');
  }
  assert.equal(qualifyOperations(null).status, 'HOLD');
});

test('each hard limit, missing path, first-use sample and completion boundary refuses', () => {
  for (const name of Object.keys(operationLimitsMs)) {
    for (const fault of ['over', 'missing', 'ack_only', 'incomplete', 'first_use', 'negative', 'nan']) {
      const packet = evidence();
      const sample = packet.operations[name][0];
      if (fault === 'over') sample.completed_ms++;
      if (fault === 'missing') packet.operations[name] = [];
      if (fault === 'ack_only') sample.boundary = 'admission_ack';
      if (fault === 'incomplete') delete sample.completed_ms;
      if (fault === 'first_use') sample.first_use = false;
      if (fault === 'negative') sample.completed_ms = 0;
      if (fault === 'nan') sample.completed_ms = NaN;
      assert.equal(qualifyOperations(packet).status, 'HOLD', `${name}: ${fault}`);
    }
  }
});

test('early ACK and favorable p95 cannot hide late completion or any exceedance', () => {
  const packet = evidence();
  const sample = packet.operations.send_complete[0];
  packet.operations.send_complete = Array.from({ length: 100 }, () => ({ ...sample, completed_ms: 200 }));
  packet.operations.send_complete.push({ ...sample, ack_ms: 101, completed_ms: 1101 });
  packet.p95_ms = 100;
  const result = qualifyOperations(packet);
  assert.equal(result.status, 'HOLD');
  assert.equal(result.operations.send_complete.maximum_ms, 1001);
  assert.deepEqual(result.operations.send_complete.exceedances, [{ index: 100, elapsed_ms: 1001 }]);
});

test('error and cancellation counts stay in the denominator and hold acceptance', () => {
  const packet = evidence();
  const samples = packet.operations.tab_open;
  samples.push({ ...samples[0], outcome: 'error' }, { ...samples[0], outcome: 'cancelled' });
  const result = qualifyOperations(packet);
  assert.equal(result.status, 'HOLD');
  assert.equal(result.operations.tab_open.samples, 3);
  for (const key of ['success', 'error', 'cancelled']) assert.equal(result.operations.tab_open[key], 1);
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
});
