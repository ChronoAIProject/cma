import assert from 'node:assert/strict';
import {performance} from 'node:perf_hooks';

export function percentile(values, fraction) {
  assert(values.length > 0 && values.every(x => Number.isFinite(x) && x >= 0));
  assert(fraction > 0 && fraction <= 1);
  return [...values].sort((a, b) => a - b)[Math.ceil(values.length * fraction) - 1];
}

export function sameHome(before, after) {
  for (const key of ['sandbox_id', 'native_uid', 'home_identity', 'pvcs']) {
    assert.ok(before[key], `missing home evidence: ${key}`);
    assert.deepEqual(after[key], before[key], `home changed: ${key}`);
  }
  assert.ok(before.pvcs.length > 0, 'home PVC evidence absent');
}

export function summarize(cycles, requested) {
  const passed = cycles.filter(x => x.verdict === 'PASS');
  const p95 = passed.length ? percentile(passed.map(x => x.resume_ms), 0.95) : null;
  return {requested, attempted: cycles.length, passed: passed.length,
    failed: cycles.filter(x => x.verdict === 'FAIL').length,
    resume_p95_ms: p95, resume_target_ms: 30000,
    verdict: passed.length === requested && cycles.length === requested && p95 <= 30000 ? 'PASS' : 'FAIL'};
}

// The adapter owns HTTP/Kubernetes details. The engine never calls manual resume
// or pause-idle-now, and records failures without silently replacing samples.
export async function campaign(adapter, {count = 100, record, now = () => performance.now()} = {}) {
  assert(Number.isInteger(count) && count > 0 && count <= 100);
  const baseline = await adapter.prepare();
  await adapter.alwaysOnException(baseline);
  const cycles = [];
  for (let index = 1; index <= count; index += 1) {
    const row = {index, started_at: new Date().toISOString(), verdict: 'FAIL'};
    try {
      row.pause = await adapter.awaitAutomaticPause(baseline);
      const start = now();
      const turn = await adapter.wakeWithMessage(baseline, index);
      row.resume_ms = now() - start;
      sameHome(baseline.home, await adapter.home(baseline));
      if (index === 1) row.active_turn = await adapter.holdActiveTurn(baseline, turn);
      row.response = await adapter.completeTurn(baseline, turn);
      await adapter.verifyRetention(baseline, row.response);
      row.verdict = 'PASS';
    } catch (error) {
      row.error = error.message;
      cycles.push(row);
      await record({cycles, summary: summarize(cycles, count)});
      throw error;
    }
    cycles.push(row);
    await record({cycles, summary: summarize(cycles, count)});
  }
  await adapter.verifyAllReceipts(baseline);
  return {cycles, summary: summarize(cycles, count), baseline};
}
