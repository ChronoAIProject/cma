import test from 'node:test';
import assert from 'node:assert/strict';
import {browser, evidence} from './common.mjs';
import {executeCase} from '../../runner.mjs';
import {validServingReceipt} from '../../serving.mjs';

const source = '2'.repeat(40), image = 'local/api@sha256:' + '1'.repeat(64);

function servingReceipt(am) {
  const snapshot = {schema: 'c275-serving-v1', am, origin: `http://127.0.0.1:${am === 'a' ? 19001 : 19002}`,
    pod_uid: `pod-${am}`, instance_id: `process-${am}`, container_id: `container-${am}`, ready: true};
  return {am, serving_am: am, method: 'GET', status: 200, request_id: `settings-read-${am}`,
    serving: {...snapshot, observed_at: '2026-11-16T10:00:00.000Z'},
    started_at: '2026-11-16T10:00:00.001Z', ended_at: '2026-11-16T10:00:00.002Z',
    serving_after: {...snapshot, observed_at: '2026-11-16T10:00:00.003Z'},
    attribution: {verdict: 'PASS', basis: 'pod_forward_before_after'}};
}

for (const lateFailure of [null, 'browser_mutation_outcome_unknown']) {
  test(`ADMIN final evidence joins browser drain: ${lateFailure ?? 'successful close'}`, async () => {
    const order = [], retained = [], receipts = [servingReceipt('a')];
    let drained = false;
    const fixture = {
      async open() {
        return {page: {}, origin: 'http://127.0.0.1:19001', switchAM: async () => {},
          async close() {
            order.push('close_started');
            // Model a response/failure discovered by asynchronous transport drain,
            // after the case's earlier successful evidence observation.
            await new Promise(resolve => setImmediate(resolve));
            receipts.push(servingReceipt('b'));
            drained = true;
            order.push('close_finished');
          }};
      },
      async evidence() {
        order.push(drained ? 'final_evidence' : 'early_evidence');
        if (drained && lateFailure) throw Error(lateFailure);
        return {schema: 'multi-am-browser-evidence-v1', real_ui: true, secrets_redacted: true,
          compiled_source: source, requests: structuredClone(receipts)};
      },
    };
    const row = {case_id: 'C275:ADM-044', family: 'admin', required_steps: ['expected_result']};
    const definition = {id: row.case_id, scope: 'browser', fixtures: ['browser'], steps: row.required_steps,
      async run(ctx) {
        const session = await browser(ctx);
        await ctx.step('expected_result', async () => {await evidence(ctx, session);});
      }};
    const result = await executeCase(definition, row, {phase: 'before_restart', iteration: 1,
      am: 'a', source, image, fixtures: {browser: fixture}, record: async event => retained.push(event)});
    assert.equal(result.steps[0].status, 'PASS', 'the earlier UI assertions alone cannot determine the verdict');
    assert.deepEqual(order, ['early_evidence', 'close_started', 'close_finished', 'final_evidence']);
    assert.equal(result.status, lateFailure ? 'FAIL' : 'PASS');
    assert.equal(result.primary_failure, null, 'cleanup failure remains distinct from product assertions');
    if (lateFailure) {
      assert.equal(result.cleanup.find(c => c.label === 'admin_browser_final_evidence').error_code, lateFailure);
      assert.ok(retained.some(e => e.kind === 'cleanup_failure' && e.error_code === lateFailure));
    } else {
      const final = retained.find(e => e.kind === 'observation' && e.label === 'admin_browser_final_evidence');
      assert.deepEqual(final.value.requests, receipts, 'full before/after identities and timing must survive capture');
      assert.deepEqual(result.requests, [servingReceipt('a'), ...receipts]);
      assert.ok(result.requests.every(validServingReceipt), 'receipt projection must preserve the shared serving proof');
      assert.equal(result.cleanup.filter(c => c.label === 'admin_browser_final_evidence').length, 1);
    }
  });
}
