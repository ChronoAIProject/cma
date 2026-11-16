import test from 'node:test';
import assert from 'node:assert/strict';
import {randomUUID} from 'node:crypto';
import {armConsumerCreation, creationReceipt, assertConsumerChannel, wizardBody} from './consumer-wizard.mjs';
import {digest} from './helpers.mjs';

const ctx = {assert: (condition, code) => assert.ok(condition, code)};
const wizard = {name: 'Synthetic consumer', connection: {ref: 'synthetic-connection'},
  image: {id: 'system-default'}, model: 'fixture-model', effort: 'high', permission: 'guarded', prompt: 'Synthetic prompt',
  skill: {id: 'synthetic-skill', version: '1.0.0'}, service: {id: 'synthetic-service'}, senders: ['synthetic-sender'],
  autoFinal: false, ackAfterMs: 1500};

async function interception(options = {}) {
  const body = wizardBody(wizard), key = randomUUID(), calls = [], cleanups = [];
  const controller = new AbortController();
  let handler;
  const page = {url: () => 'http://127.0.0.1:1234/bots/new',
    async route(matcher, operation) {
      assert.equal(matcher(new URL('http://127.0.0.1:1234/api/v1/bots')), true);
      assert.equal(matcher(new URL('http://127.0.0.1:1234/api/agent-manager/v1/bots')), true);
      assert.equal(matcher(new URL('http://127.0.0.1:4321/api/v1/bots')), false);
      handler = operation;
    }, async unroute() {calls.push('unroute');},
    async evaluate(_operation, storageKey) {
      assert.equal(storageKey, 'cma:bot-creation:["http://127.0.0.1:1234","synthetic-owner"]');
      calls.push('saved-intent');
      return options.wrongSaved ? {version: 1, intent: {key: 'different', body}}
        : {version: 1, intent: {key, body}};
    }};
  const lease = {expected: {ownerSubject: 'synthetic-owner'}, async retainCreationIntent(intent) {
    calls.push('retain'); assert.deepEqual(intent, {key, body});
    if (options.custodyFailure) throw Error('custody_failed');
    if (options.cancelAtCustody) controller.abort(Error('case_budget_exhausted'));
    return {retained: true, key_sha256: digest(key), body_sha256: digest(body)};
  }};
  const operation = await armConsumerCreation({...ctx, signal: controller.signal,
    remainingMs: () => 500, defer: (_label, work) => cleanups.push(work)}, page, lease, body);
  const dispatch = (method = 'POST') => handler({request: () => ({method: () => method,
    postDataJSON: () => options.wrongBody ? {...body, name: 'different'} : body,
    allHeaders: async () => ({'idempotency-key': key})}),
    async fallback() {calls.push('fallback');},
    async fetch(config) {
      assert.deepEqual(config, {maxRetries: 0, maxRedirects: 0, timeout: 500});
      calls.push('fetch'); if (options.fetchFailure) throw Error('upstream_unknown');
      if (options.waitFetch) await options.waitFetch;
      return {status: () => 201, json: async () => ({bot_id: 'observed-bot', state: 'creating'}),
        headers: () => ({'x-request-id': 'observed-request'}), async dispose() {calls.push('dispose');}};
    }, async abort(reason) {assert.equal(reason, 'failed'); calls.push('abort'); if (options.abortFailure) throw Error('abort_failed');}});
  return {body, key, calls, operation, dispatch, controller, cleanup: () => cleanups[0]()};
}

test('Consumer creation forwards once only after intent custody and loses only the accepted browser response', async () => {
  const f = await interception(); await f.dispatch();
  const result = await f.operation.result();
  assert.equal(result.accepted.bot_id, 'observed-bot'); assert.equal(result.key, f.key);
  assert.deepEqual(f.calls, ['saved-intent', 'retain', 'fetch', 'dispose', 'abort']);
  await f.dispatch('GET'); assert.equal(f.operation.attempts(), 1); await f.cleanup();
});

test('changed body, wrong retained intent and custody failure all abort before mutation', async () => {
  for (const option of ['wrongBody', 'wrongSaved', 'custodyFailure']) {
    const f = await interception({[option]: true}); await f.dispatch();
    await assert.rejects(f.operation.result());
    assert.ok(!f.calls.includes('fetch')); assert.equal(f.calls.at(-1), 'abort');
    await assert.rejects(f.cleanup(), /consumer_creation_route_failed/);
  }
});

test('a second browser POST is refused and retained as cleanup failure', async () => {
  const f = await interception(); await f.dispatch(); await f.operation.result(); await f.dispatch();
  assert.equal(f.calls.filter(c => c === 'fetch').length, 1);
  assert.equal(f.operation.attempts(), 2);
  await assert.rejects(f.cleanup(), error => error.errors[0].message === 'consumer_create_duplicate_attempt');
});

test('uncertain forwarding is not retried and simultaneous fault errors are preserved', async () => {
  const f = await interception({fetchFailure: true, abortFailure: true}); await f.dispatch();
  await assert.rejects(f.operation.result(), error => {
    assert.deepEqual(error.errors.map(e => e.message), ['upstream_unknown', 'abort_failed']); return true;
  });
  assert.equal(f.calls.filter(c => c === 'fetch').length, 1);
  await assert.rejects(f.cleanup());
});

test('cancellation with no POST settles; cancellation after custody refuses forwarding', async () => {
  const idle = await interception(); idle.controller.abort(Error('case_budget_exhausted'));
  await assert.rejects(idle.operation.result(), /case_budget_exhausted/); await idle.cleanup();
  const active = await interception({cancelAtCustody: true}); await active.dispatch();
  await assert.rejects(active.operation.result(), /consumer_creation_cancelled/);
  assert.ok(!active.calls.includes('fetch')); await assert.rejects(active.cleanup());
});

test('cleanup joins a pending forwarded POST before releasing route custody', async () => {
  let finish; const waitFetch = new Promise(resolve => {finish = resolve;});
  const f = await interception({waitFetch}); const pending = f.dispatch();
  while (!f.calls.includes('fetch')) await new Promise(resolve => setImmediate(resolve));
  let cleaned = false; const cleanup = f.cleanup().then(() => {cleaned = true;});
  await new Promise(resolve => setImmediate(resolve)); assert.equal(cleaned, false);
  finish(); await pending; await cleanup;
  assert.equal(f.calls.filter(c => c === 'fetch').length, 1);
});

test('Consumer creation receipt follows the actual AM and rejects label-only or ambiguous evidence', () => {
  const serving = {schema: 'c275-serving-v1', am: 'b', origin: 'http://127.0.0.1:1234', ready: true,
    pod_uid: 'fixture-pod', instance_id: 'fixture-process', container_id: 'fixture-container',
    observed_at: '2026-11-16T10:00:00.000Z'};
  const receipt = {am: 'b', request_id: 'observed-request', method: 'POST', status: 201, serving,
    serving_after: {...serving, observed_at: '2026-11-16T10:00:00.003Z'},
    started_at: '2026-11-16T10:00:00.001Z', ended_at: '2026-11-16T10:00:00.002Z',
    attribution: {verdict: 'PASS', basis: 'pod_forward_before_after'}};
  assert.equal(creationReceipt(ctx, {requests: [receipt]}, 'observed-request').am, 'b');
  for (const requests of [[{...receipt, serving: undefined}], [receipt, receipt], [{...receipt, request_id: 'different'}]])
    assert.throws(() => creationReceipt(ctx, {requests}, 'observed-request'), /actual_am_receipt/);
});

test('provisioned channel checks source admission, reply sinks and exact owner identity', () => {
  const body = wizardBody(wizard), active = {owner: 'synthetic-owner', channel: {channel_id: 'channel', record_id: 'record'}};
  const channel = {...active.channel, definition: {name: body.name, connection_ref: body.connection_ref,
    owner: {subject: active.owner}, profile: {metadata: {owner: {subject: active.owner}}},
    sources: [{type: 'telegram', admission: body.admission}], reply: {...body.reply, sinks: [{type: 'source'}]}}};
  assertConsumerChannel(ctx, channel, active, body);
  for (const mutate of [v => {v.definition.sources[0].admission = {type: 'all'};},
    v => {v.definition.reply.sinks = [];}, v => {v.definition.owner.subject = 'other-owner';}]) {
    const changed = structuredClone(channel); mutate(changed);
    assert.throws(() => assertConsumerChannel(ctx, changed, active, body), /consumer_provision_channel/);
  }
});
