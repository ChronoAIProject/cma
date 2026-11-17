import test from 'node:test';
import assert from 'node:assert/strict';
import {setImmediate as nextTurn} from 'node:timers/promises';
import {egRequest, pages} from './helpers.mjs';

// Protocol doubles exercise dispatch/custody, not product or serving evidence.
const context = {am: 'a', assert: (condition, code) => assert.ok(condition, code),
  observe: async (_label, operation) => operation()};
const replicas = [{slot: 'a', instance: 'synthetic-eg-a'}];
const response = {status: 200, body: {receipt: 'synthetic-acknowledgement'},
  instance: 'synthetic-eg-a', requestId: 'synthetic-request', eg_receipt: {scope: 'eg'}};

for (const expired of ['cancellation', 'case budget']) test(
  `EG ${expired} before dispatch starts no request`, async () => {
  const controller = new AbortController();
  if (expired === 'cancellation') controller.abort();
  let calls = 0;
  const lease = {replicas, async request() {calls++; return response;}};
  await assert.rejects(egRequest({...context, signal: controller.signal,
    remainingMs: () => expired === 'case budget' ? 0 : 1000}, lease,
  {method: 'POST', path: '/v1/synthetic', body: {intent: 'one'}}), /eg_request_budget_exhausted/);
  assert.equal(calls, 0);
});

for (const expired of ['cancellation', 'case budget']) test(
  `EG ${expired} joins the original POST and prevents a subsequent POST`, async () => {
  const controller = new AbortController();
  let unblock, notifyStarted, remaining = 1000, acknowledgement, cleanups = 0;
  const gate = new Promise(resolve => {unblock = resolve;});
  const started = new Promise(resolve => {notifyStarted = resolve;});
  const calls = [], observations = [];
  const lease = {replicas, async request(request) {
    calls.push(request); notifyStarted(); await gate; return response;
  }};
  const ctx = {...context, signal: controller.signal, remainingMs: () => remaining,
    observe: async (label, operation) => {
      const value = await operation(); observations.push({label, value}); return value;
    }};
  const execution = (async () => {
    try {
      acknowledgement = await egRequest(ctx, lease, {method: 'POST', path: '/v1/synthetic',
        key: 'original', body: {intent: 'one'}, returnResponse: true});
      await egRequest(ctx, lease, {method: 'POST', path: '/v1/synthetic',
        key: 'subsequent', body: {intent: 'two'}});
    } finally {cleanups++;}
  })().then(value => ({value}), error => ({error}));
  await started;
  if (expired === 'cancellation') controller.abort();
  else remaining = 0;
  await nextTurn();
  const earlyCleanup = cleanups;
  unblock();
  const outcome = await execution;
  assert.equal(earlyCleanup, 0, 'join the original mutation before cleanup');
  assert.equal(acknowledgement, response, 'preserve the acknowledged original result for reconciliation');
  assert.deepEqual(observations.map(item => item.label), ['eg_serving_receipt', 'eg_http_receipt']);
  assert.equal(observations[0].value.receipt, response.eg_receipt);
  assert.match(outcome.error?.message ?? '', /eg_request_budget_exhausted/);
  assert.deepEqual(calls.map(call => call.key), ['original'], 'do not dispatch or replay after expiry');
  assert.equal(cleanups, 1);
});

test('EG pagination stops before another page when the preceding read exhausts the budget', async () => {
  let remaining = 1000, calls = 0;
  const lease = {replicas, async request() {
    calls++; remaining = 0;
    return {...response, body: {items: [], next_cursor: calls === 1 ? 'next' : null}};
  }};
  await assert.rejects(pages({...context, remainingMs: () => remaining}, lease,
    '/v1/channel-records', 'record_id'), /eg_request_budget_exhausted/);
  assert.equal(calls, 1);
});

test('EG requests preserve timely responses and original transport failures without budget hooks', async () => {
  let calls = 0;
  const original = Error('synthetic_original_transport_failure');
  const lease = {replicas, async request() {
    if (++calls === 2) throw original;
    return response;
  }};
  assert.equal(await egRequest(context, lease, {path: '/v1/synthetic'}), response.body);
  await assert.rejects(egRequest(context, lease, {path: '/v1/synthetic'}), error => error === original);
  assert.equal(calls, 2);
});
