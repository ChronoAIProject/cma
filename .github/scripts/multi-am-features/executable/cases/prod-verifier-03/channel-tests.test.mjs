import test from 'node:test';
import assert from 'node:assert/strict';
import {Blocked} from '../../runner.mjs';
import {digest} from './helpers.mjs';
import {setups} from './setup.mjs';
import {channelTestsCase, validateNodeTestFixture, assertNodeTestCounts, assertSourceCheck,
  guardNodeTests, assertNodeTestTurn, findNodeTestActivity} from './channel-tests.mjs';

const ctx = {assert: (value, code) => assert.ok(value, code)};
const stamp = ms => `2026-11-16T10:00:00.${String(ms).padStart(3, '0')}Z`;
function egReceipt(requestId = 'eg-request') {
  const serving = {schema: 'c275-eg-serving-v1', scope: 'eg', replica: 'b', ready: true,
    origin: 'http://127.0.0.1:4321', pod_uid: 'pod-eg-b', instance_id: 'eg-b', container_id: 'eg-container-b',
    source: 'a'.repeat(40), image: 'eg@sha256:' + 'b'.repeat(64), observed_at: stamp(0)};
  return {scope: 'eg', replica: 'b', request_id: requestId, method: 'POST', status: 200,
    serving, serving_after: {...serving, observed_at: stamp(10)}, started_at: stamp(1), ended_at: stamp(9),
    attribution: {verdict: 'PASS', basis: 'pod_forward_before_after'}};
}
function fixture() {
  return {local: true, synthetic: true, recordId: 'record', channelId: 'channel', channelName: 'Channel',
    replicas: [{ready: true}, {ready: true}], close: async () => {},
    expected: {ownerSubject: 'owner', producerNodeName: 'Producer: Custom event',
      consumerNodeName: 'Consumer: Reviewer', sourceIndex: 3, consumerId: 'reviewer',
      channel: {agentId: 'default-agent', firstText: 'Default answered', level: 'l2'},
      consumer: {agentId: 'reviewer-agent', firstText: 'Reviewer answered', level: 'l2'}},
    request() {}, counters() {}, nodeTestCaptures() {}, quiesce() {}, drain() {}};
}

test('EG-027 preserves its frozen steps and requires real EG UI plus a named setup', () => {
  assert.equal(channelTestsCase.id, 'C275:EG-027');
  assert.deepEqual(channelTestsCase.steps, ['step_1', 'step_2', 'expected_result']);
  assert.deepEqual(channelTestsCase.fixtures, ['browser', 'user-fixtures', 'eg_browser']);
  assert.equal(channelTestsCase.evidence_scope, undefined, 'diagnostic tests start AM work');
});

test('diagnostic setup rejects missing primitives after obtaining cleanup custody', async () => {
  const lease = fixture(); let closed = 0; const cleanup = [];
  lease.close = async () => closed++;
  delete lease.nodeTestCaptures;
  await assert.rejects(setups.channel_tests({row: {}, defer: (_label, fn) => cleanup.push(fn)},
    {eg: {acquire: async () => lease}}), error => error instanceof Blocked);
  await cleanup[0](); await cleanup[0](); assert.equal(closed, 1);
});

test('diagnostic setup requires a distinct consumer and independent expected outputs', () => {
  assert.equal(validateNodeTestFixture(fixture()).sourceIndex, 3);
  for (const change of [f => f.expected.consumerId = 'default', f => f.expected.sourceIndex = -1,
    f => f.expected.consumer.firstText = f.expected.channel.firstText, f => delete f.expected.ownerSubject,
    f => delete f.drain, f => f.expected.consumer.level = 'unknown']) {
    const value = fixture(); change(value);
    assert.throws(() => validateNodeTestFixture(value), error => error instanceof Blocked);
  }
});

function sourceCheck() {
  return {source_index: 3, platform: 'custom', scope: 'configuration_only', external_delivery_tested: false,
    checks: [
      {id: 'configuration', label: 'Saved configuration', status: 'passed',
        detail: 'The saved channel definition passes schema and policy validation.'},
      {id: 'credential', label: 'Authentication configuration', status: 'passed',
        detail: 'The configured authentication references are present; caller credentials were not tested.'},
      {id: 'external_delivery', label: 'External delivery', status: 'unavailable',
        detail: 'Send a message from the configured platform to verify incoming delivery. This check sends no external messages.'},
    ]};
}

test('source checks reject the wrong index, missing credentials and claimed external delivery', () => {
  assertSourceCheck(ctx, sourceCheck(), 3);
  for (const change of [v => v.source_index = 0, v => v.external_delivery_tested = true,
    v => v.checks[1].status = 'unavailable', v => v.checks[2].status = 'passed', v => v.checks.pop(),
    v => v.checks[0].detail = 'Generic success', v => v.scope = 'platform_delivery']) {
    const value = sourceCheck(); change(value); assert.throws(() => assertSourceCheck(ctx, value, 3));
  }
});

test('per-action counters reject cancelling side effects hidden by an aggregate total', () => {
  const before = {turns: 4, effects: 4, deliveries: 2, ingress: 8};
  assertNodeTestCounts(ctx, before, before, 0);
  assertNodeTestCounts(ctx, before, {...before, turns: 5, effects: 5}, 1);
  assert.throws(() => assertNodeTestCounts(ctx, before, {...before, turns: 5, effects: 5}, 0));
  assert.throws(() => assertNodeTestCounts(ctx, before, before, 1));
  for (const change of [v => v.deliveries++, v => v.ingress++, v => delete v.effects]) {
    const value = {...before}; change(value); assert.throws(() => assertNodeTestCounts(ctx, before, value, 0));
  }
});

async function guarded(options = {}) {
  const calls = [], cleanup = [], observations = [], receipts = [], origin = 'http://127.0.0.1:4321';
  const controller = new AbortController();
  const page = {
    async route(matcher, handler) {this.matcher = matcher; this.handler = handler;},
    async unroute() {calls.push('unroute');},
    async send(input = {}) {
      const path = input.path ?? '/api/event-gateway/v1/channels/channel/sources/3/test';
      const response = {status: () => options.status ?? 200,
        headers: () => ({'x-request-id': 'eg-request'}), json: async () => sourceCheck(),
        dispose: async () => {calls.push('dispose'); if (options.disposeError) throw Error('dispose_failed');}};
      const request = {url: () => origin + path, method: () => input.method ?? 'POST',
        postData: () => input.body ?? null, allHeaders: async () => input.headers ?? {}};
      await this.handler({request: () => request, fallback: async () => calls.push('fallback'),
        fetch: async settings => {
          calls.push(['fetch', settings]);
          if (options.hold) await options.hold;
          receipts.push(egReceipt(options.wrongReceipt ? 'different-request' : 'eg-request'));
          return response;
        }, fulfill: async () => calls.push('fulfill'), abort: async () => calls.push('abort')});
    },
  };
  const context = {...ctx, signal: controller.signal, remainingMs: () => options.budget ?? 1000,
    defer: (_label, fn) => cleanup.push(fn), observe: async (_label, read) => {
      const value = await read(); observations.push(value); return value;
    }};
  const session = {page, origin, networkIdle: async () => {}, egEvidence: async () => receipts};
  const guard = await guardNodeTests(context, session);
  const run = (input, spec = {}) => guard.run({name: 'source', path: '/v1/channels/channel/sources/3/test', ...spec},
    () => page.send(input));
  return {guard, page, calls, cleanup, controller, run, observations};
}

test('test guard forwards exactly once with no retries and joins the actual EG receipt', async () => {
  const f = await guarded(); const result = await f.run();
  assert.equal(result.receipt.replica, 'b'); assert.equal(result.requestId, 'eg-request');
  assert.equal(f.guard.forwarded(), 1);
  assert.deepEqual(f.calls[0], ['fetch', {maxRetries: 0, maxRedirects: 0, timeout: 1000}]);
  assert.deepEqual(f.calls.slice(1), ['fulfill', 'dispose']);
  assert.equal(f.observations[0].schema, 'multi-eg-request-v1');
  await f.cleanup[0]();
});

test('accepted-response loss aborts downstream only after real acceptance and disposes the response', async () => {
  const f = await guarded(); const result = await f.run({}, {drop: true});
  assert.equal(result.dropped, true); assert.deepEqual(result.body, sourceCheck());
  assert.deepEqual(f.calls.slice(1), ['abort', 'dispose']);
  await assert.rejects(f.run(), /node_test_operation_once/);
  assert.equal(f.guard.forwarded(), 1); await f.cleanup[0]();
});

test('wrong diagnostic endpoint, mutation type, query, body and replay key never reach EG', async () => {
  for (const input of [
    {path: '/api/event-gateway/v1/channels/channel/sources/0/test'},
    {path: '/api/event-gateway/v1/channels/channel/sources/3/test?retry=1'},
    {method: 'DELETE'}, {body: '{}'}, {headers: {'idempotency-key': 'replacement-key'}},
  ]) {
    const f = await guarded(); await assert.rejects(f.run(input), /node_test_mutation_failed/);
    assert.equal(f.guard.forwarded(), 0); await assert.rejects(f.cleanup[0](), /node_test_guard_cleanup_failed/);
  }
});

test('duplicate and unarmed EG writes remain failures without a second forward', async () => {
  const f = await guarded();
  await assert.rejects(f.guard.run({name: 'source', path: '/v1/channels/channel/sources/3/test'},
    () => Promise.all([f.page.send(), f.page.send()])), /node_test_mutation_failed/);
  assert.equal(f.guard.forwarded(), 1); await assert.rejects(f.cleanup[0](), /node_test_guard_cleanup_failed/);
  const unarmed = await guarded(); await unarmed.page.send();
  assert.equal(unarmed.guard.forwarded(), 0); assert.throws(() => unarmed.guard.check(), /node_test_mutation_failed/);
  await assert.rejects(unarmed.cleanup[0](), /node_test_guard_cleanup_failed/);
});

test('missing serving attribution and response-disposal failure cannot produce success', async () => {
  const wrong = await guarded({wrongReceipt: true});
  await assert.rejects(wrong.run(), /channel_actual_eg_browser_receipt/); await wrong.cleanup[0]();
  const badDisposal = await guarded({disposeError: true});
  await assert.rejects(badDisposal.run(), /node_test_mutation_failed/);
  await assert.rejects(badDisposal.cleanup[0](), /node_test_guard_cleanup_failed/);
});

test('cancelled diagnostics keep in-flight work joined until its response is disposed', async () => {
  let release; const hold = new Promise(resolve => {release = resolve;});
  const f = await guarded({hold});
  // Cancellation must reject even when the browser action itself is waiting on
  // the held request. Cleanup still joins and disposes that same request.
  let inFlight;
  const running = f.guard.run({name: 'source', path: '/v1/channels/channel/sources/3/test'},
    () => {inFlight = f.page.send(); return inFlight;});
  await new Promise(resolve => setImmediate(resolve)); f.controller.abort();
  await assert.rejects(running, /node_test_cancelled_or_expired/);
  let cleaned = false; const cleanup = f.cleanup[0]().then(() => {cleaned = true;});
  await new Promise(resolve => setImmediate(resolve)); assert.equal(cleaned, false);
  release(); await inFlight; await cleanup;
  assert.equal(f.guard.forwarded(), 1); assert.ok(f.calls.includes('dispose'));
});

test('a UI action that sends no diagnostic expires without leaving a live waiter', async () => {
  const f = await guarded({budget: 5});
  await assert.rejects(f.guard.run({name: 'source', path: '/v1/channels/channel/sources/3/test'},
    async () => {}), /node_test_cancelled_or_expired/);
  assert.equal(f.guard.forwarded(), 0); await f.cleanup[0]();
});

function turn() {
  const agentId = 'reviewer-agent', serving = {schema: 'c275-serving-v1', am: 'a', ready: true,
    origin: 'http://127.0.0.1:10001', pod_uid: 'am-pod', instance_id: 'am-instance',
    container_id: 'am-container', observed_at: stamp(2)};
  return {eg_request_id: 'eg-request', channel_id: 'channel', consumer_id: 'reviewer', agent_id: agentId,
    binding_id: 'binding', conversation_id: 'diagnostic-conversation', response_id: 'actual-response',
    started_at: stamp(2), accepted_at: stamp(5), completed_at: stamp(8), first_text: 'Reviewer answered',
    admission_request_id: 'am-request', amRequests: [{am: 'a', method: 'POST', status: 202,
      elapsed_ms: 2, request_id: 'am-request', agent_id_sha256: digest(agentId), serving,
      serving_after: {...serving, observed_at: stamp(5)}, started_at: stamp(3), ended_at: stamp(4),
      attribution: {verdict: 'PASS', basis: 'pod_forward_before_after'}, owner_proof: {
        verdict: 'PASS', kind: 'non_owner', agent_sha256: digest(agentId), request_id: 'am-request',
        ingress: 'a', owner: 'b', owner_epoch: 2, ingress_instance_sha256: digest('am-instance'),
        owner_instance_sha256: digest('other-am-instance')}}]};
}
const result = () => ({requestId: 'eg-request', receipt: egReceipt(),
  body: {accepted_after_ms: 3, level: 'l2', first_text: 'Reviewer answered'}});
const expected = {channelId: 'channel', consumerId: 'reviewer', agentId: 'reviewer-agent',
  bindingId: 'binding', level: 'l2', firstText: 'Reviewer answered'};

test('diagnostic turns require real non-owner admission and exact selected-consumer joins', () => {
  assertNodeTestTurn(ctx, turn(), result(), expected);
  for (const change of [v => v.eg_request_id = 'other', v => v.consumer_id = 'default',
    v => v.binding_id = 'other-binding', v => v.first_text = 'Default answered',
    v => v.admission_request_id = 'another-post', v => v.completed_at = stamp(100),
    v => delete v.amRequests[0].serving_after, v => v.amRequests[0].owner_proof.owner = 'a',
    v => v.amRequests[0].owner_proof.owner_epoch = 0, v => v.amRequests[0].method = 'GET',
    v => v.amRequests[0].status = 503, v => v.amRequests.push(structuredClone(v.amRequests[0]))]) {
    const value = turn(); change(value); assert.throws(() => assertNodeTestTurn(ctx, value, result(), expected));
  }
  for (const change of [v => v.body.accepted_after_ms = -1, v => v.body.first_text = 'Wrong node',
    v => v.body.level = 'l5']) {
    const value = result(); change(value); assert.throws(() => assertNodeTestTurn(ctx, turn(), value, expected));
  }
});

function activity() {
  const diagnostic = turn();
  return {cursor: 'actual-cursor', retention_days: 7, limit: 20, has_more: false, items: [{
    event_id: 'durable-diagnostic', conversation: {id: diagnostic.conversation_id}, source: {}, actor: {},
    received_at: stamp(2), occurred_at: stamp(2), consumers: [{
      consumer_id: diagnostic.consumer_id, agent_id: diagnostic.agent_id, response_id: diagnostic.response_id,
      conversation_id: diagnostic.conversation_id, state: 'completed', accepted_at: stamp(5),
      completed_at: stamp(8), deliveries: [],
    }],
  }]};
}

test('unknown diagnostics remain BLOCKED without a matching durable Activity result', () => {
  const snapshot = activity(); findNodeTestActivity(ctx, snapshot, turn());
  for (const change of [v => v.items = [], v => v.items[0].consumers[0].response_id = 'unrelated',
    v => v.items[0].consumers[0].agent_id = 'unrelated', v => v.items[0].consumers[0].consumer_id = 'default',
    v => v.items[0].consumers[0].conversation_id = 'other-conversation']) {
    const value = activity(); change(value);
    assert.throws(() => findNodeTestActivity(ctx, value, turn()), error => error instanceof Blocked
      && /node_test_unknown_activity_reconciliation_missing/.test(error.message));
  }
  snapshot.items[0].consumers[0].state = 'accepted';
  assert.throws(() => findNodeTestActivity(ctx, snapshot, turn()), /node_test_activity_completed/);
});
