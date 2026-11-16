import test from 'node:test';
import assert from 'node:assert/strict';
import {randomUUID} from 'node:crypto';
import {armManagedConnectionAttachment, assertManagedConnectionIntent, assertManagedChannel,
  connectionCases} from './connections.mjs';
import {digest, same} from './helpers.mjs';

const expected = {ownerSubject: 'synthetic-owner', connectionRef: 'connection-ready',
  connectionLabel: 'Ready Telegram', connectionAdmission: {type: 'allowlist', sender_ids: ['sender-one']},
  boundConnectionRef: 'connection-bound', boundRecordId: randomUUID(),
  notReadyConnectionRef: 'connection-pending', foreignConnectionRef: 'connection-foreign', routeId: 'nyx-route'};
const desired = {schema_version: 1, name: 'Managed connection channel',
  profile: {metadata: {owner: {kind: 'human', subject: expected.ownerSubject}}},
  provider: {slug: 'cma_codex'}, sources: [], connection_ref: expected.connectionRef,
  connection_admission: expected.connectionAdmission, partition: {version: 1, key: 'conversation_and_sender'},
  reply: {auto_final: true, ack_after_ms: 1000, sinks: [{type: 'source'}]}};
const credential = 'synthetic-provider-key';
const hashes = {agent: `sha256:${digest(credential)}`};

function record(recordId, channelId = 'channel-created') {
  return {record_id: recordId, channel_id: channelId, version: 1,
    definition: {connection_ref: expected.connectionRef,
      profile: {metadata: {owner: {subject: expected.ownerSubject}}},
      sources: [{type: 'telegram', admission: expected.connectionAdmission}],
      name: desired.name}};
}

function context() {
  const cleanups = [], observations = [];
  return {assert: (condition, code) => assert.ok(condition, code), signal: new AbortController().signal,
    remainingMs: () => 1000, defer: (_label, work) => cleanups.push(work),
    observe: async (_label, read) => observations.push(await read()), cleanups, observations};
}

async function interception(options = {}) {
  const ctx = context(), calls = [], key = randomUUID(), recordId = randomUUID();
  const body = {...desired, record_id: recordId, credentials: {agent_key: credential}};
  const page = {url: () => 'http://127.0.0.1:1234/event-gateway', async route(matcher, handler) {
    assert.equal(matcher(new URL('http://127.0.0.1:1234/api/event-gateway/v1/channels')), true);
    assert.equal(matcher(new URL('http://127.0.0.1:4321/api/event-gateway/v1/channels')), false);
    this.handler = handler;
  }, async unroute() {calls.push('unroute');}};
  const lease = {async retainChannelIntent(intent) {
    calls.push('custody'); assert.equal(intent.recordId, recordId); assert.equal(intent.key, key);
    return {retained: true, record_sha256: digest(recordId), key_sha256: digest(key), body_sha256: digest(body)};
  }};
  const operation = await armManagedConnectionAttachment(ctx, page, lease, expected, desired, hashes);
  const dispatch = async (actualBody = body, method = 'POST') => page.handler({request: () => ({
    method: () => method, postDataJSON: () => actualBody, allHeaders: async () => ({'idempotency-key': key})}),
  async fallback() {calls.push('fallback');}, async fetch(config) {
    calls.push('fetch'); assert.deepEqual(config, {maxRetries: 0, maxRedirects: 0, timeout: 1000});
    if (options.fetchFailure) throw Error('upstream_unknown');
    return {status: () => 201, json: async () => record(recordId),
      headers: () => ({'x-request-id': 'eg-request'}), async dispose() {calls.push('dispose');}};
  }, async fulfill() {calls.push('fulfill');}, async abort(reason) {calls.push('abort'); assert.equal(reason, 'failed');}});
  return {ctx, calls, body, key, operation, dispatch};
}

test('EG-044 is browser/EG-only and preserves the frozen step IDs', () => {
  const definition = connectionCases.find(value => value.id === 'C275:EG-044');
  assert.equal(definition.evidence_scope, 'eg_only');
  assert.deepEqual(definition.steps, ['step_1', 'step_2', 'expected_result']);
  assert.ok(definition.fixtures.includes('eg_browser'));
});

test('managed connection intent requires generated UUIDs, exact admission and no route IDs', () => {
  const body = {...desired, record_id: randomUUID(), credentials: {agent_key: credential}};
  assertManagedConnectionIntent(context(), body, randomUUID(), expected, desired, hashes);
  for (const mutate of [value => {value.connection_ref = 'nyx-route';},
    value => {value.sources = [{type: 'nyxid_relay'}];},
    value => {value.connection_admission = {type: 'all'};},
    value => {value.credentials.agent_key = 'different';}]) {
    const changed = structuredClone(body); mutate(changed);
    assert.throws(() => assertManagedConnectionIntent(context(), changed, randomUUID(), expected, desired, hashes),
      /managed_connection_/);
  }
});

test('browser managed connection POST is retained, forwarded once and returned to the UI', async () => {
  const f = await interception(); await f.dispatch();
  const result = await f.operation.result();
  assert.equal(result.record.channel_id, 'channel-created');
  assert.equal(result.intent.recordId, f.body.record_id);
  assert.deepEqual(f.calls, ['custody', 'fetch', 'fulfill', 'dispose']);
  await f.dispatch();
  assert.equal(f.operation.attempts(), 2);
  await assert.rejects(f.ctx.cleanups[0](), /managed_connection_route_cleanup_failed/);
  assert.ok(f.ctx.observations.every(value => !JSON.stringify(value).includes(credential)));
});

test('changed browser body or upstream failure never forwards a second mutation', async () => {
  const f = await interception();
  const changed = {...f.body, connection_ref: 'different'};
  await f.dispatch(changed); await assert.rejects(f.operation.result(), /managed_connection_/);
  assert.ok(!f.calls.includes('fetch')); await assert.rejects(f.ctx.cleanups[0]());
  const failed = await interception({fetchFailure: true}); await failed.dispatch();
  await assert.rejects(failed.operation.result(), /upstream_unknown/);
  assert.equal(failed.calls.filter(value => value === 'fetch').length, 1);
});

test('expanded managed response retains the selected Telegram source and redacts credentials', () => {
  const intent = {recordId: randomUUID(), configuration: desired};
  assertManagedChannel(context(), record(intent.recordId), intent, expected);
  const leaked = structuredClone(record(intent.recordId)); leaked.definition.credentials = {agent_key: credential};
  assert.throws(() => assertManagedChannel(context(), leaked, intent, expected), /managed_connection_created_no_route/);
});

test('connection pagination accepts the API next_after cursor without changing channel cursor semantics', async () => {
  const {pages} = await import('./helpers.mjs');
  const observations = [], calls = []; let index = 0;
  const lease = {replicas: [{slot: 'a', instance: 'eg-a'}], request: async request => {
    calls.push(request.path); return {status: 200, instance: 'eg-a', requestId: `request-${++index}`,
      body: index === 1 ? {items: [{connection_ref: 'one'}], next_after: 'cursor-1'} : {items: [{connection_ref: 'two'}]}};
  }};
  const ctx = {am: 'a', assert: (condition, code) => assert.ok(condition, code), observe: async (_label, read) => observations.push(await read())};
  assert.deepEqual(await pages(ctx, lease, '/v1/channel-connections', 'connection_ref',
    {replica: 'a', cursorField: 'next_after'}), [{connection_ref: 'one'}, {connection_ref: 'two'}]);
  assert.deepEqual(calls, ['/v1/channel-connections', '/v1/channel-connections?after=cursor-1']);
});
