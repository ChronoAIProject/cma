import assert from 'node:assert/strict';
import {performance} from 'node:perf_hooks';
import {isNativeSuspended} from './lifecycle.mjs';

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
  assert(Number.isInteger(requested) && requested > 0 && requested <= 100);
  const passed = cycles.filter(x => x.verdict === 'PASS');
  const p95 = passed.length ? percentile(passed.map(x => x.resume_ms), 0.95) : null;
  return {scope: 'CYCLE_SAMPLE', qualification: 'NOT_ASSESSED',
    requested, attempted: cycles.length, passed: passed.length,
    failed: cycles.filter(x => x.verdict === 'FAIL').length,
    resume_p95_ms: p95, resume_target_ms: 30000,
    verdict: passed.length === requested && cycles.length === requested && p95 <= 30000 ? 'PASS' : 'FAIL'};
}

export function validateLocalOrigins(origins) {
  const local = value => {
    try {
      const url = new URL(value);
      return url.protocol === 'http:' && ['127.0.0.1', 'localhost', '[::1]'].includes(url.hostname)
        && !url.username && !url.password && url.pathname === '/' && !url.search && !url.hash;
    } catch { return false; }
  };
  assert(origins?.ams && Object.keys(origins.ams).length > 0
    && Object.values(origins.ams).every(local), 'local_api_required');
  assert(local(origins.control_origin), 'local_control_required');
}

// The adapter owns HTTP/Kubernetes details. The engine never calls manual resume
// or pause-idle-now, and records failures without silently replacing samples.
export async function campaign(adapter, {count = 100, record, now = () => performance.now()} = {}) {
  assert(Number.isInteger(count) && count > 0 && count <= 100);
  assert.equal(typeof record, 'function', 'private_recorder_required');
  const methods = ['validate', 'prepare', 'alwaysOnException', 'awaitAutomaticPause',
    'wakeWithMessage', 'home', 'holdActiveTurn', 'completeTurn', 'verifyRetention', 'verifyAllReceipts'];
  assert(methods.every(key => typeof adapter?.[key] === 'function'), 'adapter_contract_incomplete');
  validateLocalOrigins(adapter.origins);
  const cycles = [];
  const retain = async packet => {
    try { await record(structuredClone(packet)); }
    catch { throw Error('evidence_write_failed'); }
  };
  let baseline;
  try {
    // Endpoint, fixture authority and candidate checks precede any setup effect.
    await adapter.validate();
    baseline = await adapter.prepare();
    await adapter.alwaysOnException(baseline);
  } catch {
    await retain({cycles, summary: summarize(cycles, count), verdict: 'FAIL',
      final_verification: 'NOT_RUN', error_code: 'preparation_failed'});
    throw Error('preparation_failed');
  }
  for (let index = 1; index <= count; index += 1) {
    const row = {index, started_at: new Date().toISOString(), verdict: 'FAIL'};
    let stage = 'automatic_pause';
    try {
      row.pause = await adapter.awaitAutomaticPause(baseline);
      assert(isNativeSuspended(row.pause), 'runtime_suspension_unproved');
      stage = 'message_wake';
      const start = now();
      const turn = await adapter.wakeWithMessage(baseline, index);
      row.resume_ms = now() - start;
      assert(Number.isFinite(row.resume_ms) && row.resume_ms >= 0, 'resume_measurement_invalid');
      stage = 'home';
      sameHome(baseline.home, await adapter.home(baseline));
      if (index === 1) {
        stage = 'active_turn';
        row.active_turn = await adapter.holdActiveTurn(baseline, turn);
        assert(row.active_turn?.interrupted === false && Number.isSafeInteger(row.active_turn.samples)
          && row.active_turn.samples > 0 && Number.isFinite(row.active_turn.hold_ms)
          && row.active_turn.hold_ms > 0, 'active_turn_protection_unproved');
      }
      stage = 'complete_turn';
      row.response = await adapter.completeTurn(baseline, turn);
      stage = 'retention';
      await adapter.verifyRetention(baseline, row.response);
      row.verdict = 'PASS';
    } catch {
      // Provider errors may include private response bodies or credentials.
      row.error_code = stage + '_failed';
      cycles.push(row);
      await retain({cycles, summary: summarize(cycles, count), verdict: 'FAIL', final_verification: 'NOT_RUN'});
      throw Error(row.error_code);
    }
    cycles.push(row);
    await retain({cycles, summary: summarize(cycles, count), verdict: 'RUNNING', final_verification: 'PENDING'});
  }
  try { await adapter.verifyAllReceipts(baseline); }
  catch {
    await retain({cycles, summary: summarize(cycles, count), verdict: 'FAIL',
      final_verification: 'FAIL', error_code: 'final_receipt_verification_failed'});
    throw Error('final_receipt_verification_failed');
  }
  const summary = summarize(cycles, count);
  const result = {cycles, summary, verdict: summary.verdict, final_verification: 'PASS'};
  await retain(result);
  return {...result, baseline};
}
