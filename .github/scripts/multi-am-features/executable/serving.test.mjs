import test from 'node:test';
import assert from 'node:assert/strict';
import {servingBinding, requireServingReceipt, validServingReceipt} from './serving.mjs';

test('selected labels cannot replace stable process identity and an actual response window', () => {
  const serving = {schema: 'c275-serving-v1', am: 'b', ready: true, origin: 'http://127.0.0.1:10002',
    pod_uid: 'pod-b', instance_id: 'instance-b', container_id: 'container-b', observed_at: '2026-11-16T10:00:00.000Z'};
  const receipt = {am: 'b', request_id: 'actual-response', serving,
    started_at: '2026-11-16T10:00:00.001Z', ended_at: '2026-11-16T10:00:00.002Z',
    serving_after: {...serving, observed_at: '2026-11-16T10:00:00.003Z'},
    attribution: {verdict: 'PASS', basis: 'pod_forward_before_after'}};
  assert.equal(validServingReceipt(receipt), true);
  for (const change of [r => {delete r.serving;}, r => r.am = 'a',
    r => r.serving_after.pod_uid = 'new-pod', r => r.serving_after.container_id = 'new-container',
    r => r.serving_after.origin = 'http://127.0.0.1:10001', r => r.request_id = '',
    r => r.serving.ready = false, r => r.serving_after.ready = false,
    r => r.started_at = '2026-11-16T09:59:59.000Z', r => r.attribution.verdict = 'UNPROVED']) {
    const altered = structuredClone(receipt); change(altered);
    assert.equal(validServingReceipt(altered), false);
  }
  const response = {status: 202, body: {id: 'original-receipt'}, timing: {...receipt, request_id: ''}};
  assert.throws(() => requireServingReceipt(serving, response), error => error.response === response);
  assert.throws(() => servingBinding('a', {ams: {a: {...serving, origin: 'https://external.invalid'}}}), /forward_origin/);
});
