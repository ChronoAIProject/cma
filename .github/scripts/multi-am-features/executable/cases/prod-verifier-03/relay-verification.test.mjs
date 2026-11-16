import test from 'node:test';
import assert from 'node:assert/strict';
import {Blocked} from '../../runner.mjs';
import {digest} from './helpers.mjs';
import {setups} from './setup.mjs';
import {relayVerificationCase, validateRelayFixture, assertRelayConfiguration, assertRelayResponse,
  assertRelayCapture, assertRelayReplay, assertRelayCounters, assertRetainedRelay} from './relay-verification.mjs';

const ctx = {assert: (value, code) => assert.ok(value, code)};
function fixture() {
  return {local: true, synthetic: true, replicas: [{ready: true}, {ready: true}],
    recordId: 'record', channelId: 'channel', expected: {issuer: 'synthetic-issuer', keyId: 'synthetic-key-id',
      ownerSubject: 'synthetic-owner', consumerId: 'default', replySender: 'synthetic-bot', botId: '100',
      botUsername: 'synthetic_bot', allowedSender: '101', deniedSender: '102', privateChat: '101', groupChat: '-104',
      privateRoute: 'private-route', groupRoute: 'group-route', deniedRoute: 'foreign-route'},
    ...Object.fromEntries(['request', 'platformSend', 'replayEvent', 'callbackCapture', 'waitForTurn',
      'releaseCompletion', 'waitForDelivery', 'counters', 'quiesce', 'drain', 'close'].map(key => [key, async () => {}]))};
}
function record() {
  const e = fixture().expected;
  return {record_id: 'record', channel_id: 'channel', provider: {slug: 'cma_codex'}, definition: {
    owner: {subject: e.ownerSubject}, sources: [{type: 'nyxid_relay', platform: 'telegram',
      issuer: e.issuer, key_id: e.keyId, bot_id: e.botId, bot_username: e.botUsername,
      route_ids: [e.privateRoute, e.groupRoute], admission: {type: 'scoped',
        senders: {type: 'allowlist', ids: [e.allowedSender]}, chats: {type: 'allowlist', ids: [e.privateChat, e.groupChat]},
        groups: 'mention'}}], partition: {version: 1, key: 'conversation_and_sender'},
    reply: {auto_final: true, ack_after_ms: 0, sinks: [{type: 'source'}]}}};
}
function ingress(status = 202, body = {status: 'accepted', event_id: 'event', conversation_id: 'conversation'}) {
  const serving = {schema: 'c275-eg-serving-v1', scope: 'eg', replica: 'a', origin: 'http://127.0.0.1:12345',
    ready: true, pod_uid: 'pod-a', instance_id: 'instance-a', container_id: 'container-a',
    source: 'a'.repeat(40), image: 'eg@sha256:' + 'b'.repeat(64), observed_at: '2026-11-17T00:00:00.000Z'};
  return {status, body, contentType: status === 401 ? 'application/problem+json' : 'application/json', protocolVersion: '1',
    receipt: {scope: 'eg', replica: 'a', request_id: 'request-a', method: 'POST', status, serving,
      serving_after: {...serving, observed_at: '2026-11-17T00:00:00.003Z'},
      started_at: '2026-11-17T00:00:00.001Z', ended_at: '2026-11-17T00:00:00.002Z',
      attribution: {verdict: 'PASS', basis: 'pod_forward_before_after'}}};
}
const input = {messageId: 'event', correlationId: 'correlation', routeId: 'private-route',
  chatId: '101', senderId: '101', isGroup: false, mentioned: false, text: 'Synthetic relay text'};
function capture() {
  return {schema: 'multi-eg-relay-wire-v1', request_id: 'request-a', method: 'POST', path: '/v1/callbacks/channel',
    ...Object.fromEntries(['artifact_sha256', 'body_sha256', 'signed_headers_sha256', 'callback_token_sha256',
      'reply_token_sha256', 'callback_body_sha256'].map(key => [key, digest(key === 'callback_body_sha256' ? 'body_sha256' : key)])),
    facts: {platform: 'telegram', issuer: 'synthetic-issuer', keyId: 'synthetic-key-id', ...input,
      text: undefined, isBotSender: false, textDigest: digest(input.text)}};
}
const exactCapture = () => {const value = capture(); delete value.facts.text; return value;};

test('EG-035 keeps frozen steps and blocks incomplete named setup before dispatch', async () => {
  assert.equal(relayVerificationCase.id, 'C275:EG-035');
  assert.deepEqual(relayVerificationCase.steps, ['step_1', 'step_2', 'expected_result']);
  assert.deepEqual(relayVerificationCase.fixtures, ['user-fixtures']);
  validateRelayFixture(fixture());
  for (const method of ['callbackCapture', 'releaseCompletion', 'quiesce', 'request']) {
    const lease = fixture(); delete lease[method];
    assert.throws(() => validateRelayFixture(lease), error => error instanceof Blocked);
  }
  const cleanups = []; let closed = 0;
  const lease = fixture(); lease.expected.deniedRoute = lease.expected.privateRoute; lease.close = async () => closed++;
  await assert.rejects(setups.relay_verification({...ctx, row: {case_id: 'C275:EG-035'},
    defer: (_label, fn) => cleanups.push(fn)}, {eg: {acquire: async () => lease}}), error => error instanceof Blocked);
  await cleanups[0](); await cleanups[0](); assert.equal(closed, 1);
});

test('relay policy requires verified exact routes, sender/chat bounds and mention-only groups', () => {
  assertRelayConfiguration(ctx, fixture(), record());
  for (const change of [r => r.definition.sources[0].trust_normalized = true,
    r => r.definition.sources[0].route_ids.push('foreign-route'),
    r => r.definition.sources[0].admission.senders = {type: 'open'},
    r => r.definition.sources[0].admission.groups = 'all',
    r => r.definition.reply.ack_after_ms = 3000, r => r.definition.owner.subject = 'foreign',
    r => r.provider.slug = 'stub', r => delete r.definition]) {
    const value = record(); change(value);
    assert.throws(() => assertRelayConfiguration(ctx, fixture(), value), /relay_/);
  }
});

test('callback signature and route rejection cannot pass as generic 202 refusals', () => {
  for (const detail of ['token_signature', 'route_denied']) {
    const expected = {detail}, response = ingress(401, {status: 401, code: 'callback_invalid', detail});
    assertRelayResponse(ctx, response, 'a', expected);
    for (const wrong of [ingress(202, {status: 'refused', reason: detail}),
      ingress(401, {...response.body, detail: 'token_time'}), ingress(403, response.body)])
      assert.throws(() => assertRelayResponse(ctx, wrong, 'a', expected), /unauthenticated_rejection/);
  }
  for (const reason of ['sender_denied', 'group_not_addressed']) {
    assertRelayResponse(ctx, ingress(202, {status: 'refused', reason}), 'a', {reason});
    assert.throws(() => assertRelayResponse(ctx, ingress(202, {status: 'refused', reason: 'bot_sender'}), 'a', {reason}),
      /authenticated_refusal/);
  }
  const expected = {eventId: 'event', conversationId: 'conversation'};
  assertRelayResponse(ctx, ingress(), 'a', expected);
  assert.throws(() => assertRelayResponse(ctx, ingress(), 'b', expected), /actual_callback_receipt/);
  assert.throws(() => assertRelayResponse(ctx, ingress(), 'a', {...expected, eventId: 'different'}), /accepted_identity/);
  const missing = ingress(); delete missing.receipt.serving_after;
  assert.throws(() => assertRelayResponse(ctx, missing, 'a', expected), /actual_callback_receipt/);
});

test('callback capture binds actual request, signed body and raw sender/group facts', () => {
  assertRelayCapture(ctx, exactCapture(), ingress(), input, fixture());
  for (const change of [v => v.request_id = 'other', v => v.path = '/wrong',
    v => v.facts.senderId = '102', v => v.facts.isGroup = true, v => v.facts.mentioned = true,
    v => v.facts.textDigest = digest('changed'), v => delete v.artifact_sha256,
    v => v.callback_body_sha256 = digest('wrong-body')]) {
    const value = exactCapture(); change(value);
    assert.throws(() => assertRelayCapture(ctx, value, ingress(), input, fixture()), /relay_/);
  }
});

test('replayed identity alone cannot substitute for identical signed callback bytes', () => {
  const first = exactCapture(), replay = {...first, request_id: 'request-b'};
  assertRelayReplay(ctx, first, replay);
  assert.throws(() => assertRelayReplay(ctx, first, first), /distinct_http_request/);
  for (const field of ['body_sha256', 'signed_headers_sha256', 'callback_token_sha256', 'reply_token_sha256'])
    assert.throws(() => assertRelayReplay(ctx, first, {...replay, [field]: digest('re-signed')}), /same_signed_/);
  assert.throws(() => assertRelayReplay(ctx, first, {...replay, facts: {...first.facts, correlationId: 'new'}}), /same_signed_facts/);
});

test('denied or duplicate events cannot hide extra work, pending effects or late deliveries', () => {
  const settled = {turns: 2, effects: 2, deliveries: 2, pending: 0, unknown: 0, lateEffects: 0};
  assertRelayCounters(ctx, settled, 2, 2);
  assertRelayCounters(ctx, {...settled, deliveries: 1, pending: 1}, 2, 1, true);
  for (const field of Object.keys(settled))
    assert.throws(() => assertRelayCounters(ctx, {...settled, [field]: settled[field] + 1}, 2, 2), /exact_work_counts/);
});

test('retained activity proves one exact completed consumer and reply per admitted event', () => {
  const at = '2026-11-17T00:00:00Z', event = {eventId: 'event', consumerId: 'default', agentId: 'agent', responseId: 'response'};
  const page = {cursor: 'bounded-cursor', retention_days: 7, limit: 100, has_more: false,
    items: [{event_id: 'event', actor: {id: '101'}, conversation: {id: '101', kind: 'private'},
      source: {type: 'nyxid_relay', platform: 'telegram', route_id: 'private-route'}, received_at: at, occurred_at: at,
      consumers: [{consumer_id: 'default', agent_id: 'agent', response_id: 'response', conversation_id: 'agent-conversation',
        state: 'completed', dispatched_at: at, accepted_at: at, produced_at: at, completed_at: at,
        deliveries: [{step: 'answer', outcome: 'confirmed', observed_at: at, confirmed_at: at}]}]}]};
  assertRetainedRelay(ctx, page, [{event, input}]);
  for (const change of [v => v.items.push(structuredClone(v.items[0])),
    v => v.items[0].actor.id = '102', v => v.items[0].conversation.id = '-104',
    v => v.items[0].consumers[0].response_id = 'another-response',
    v => v.items[0].consumers[0].state = 'accepted',
    v => v.items[0].consumers[0].deliveries.push(structuredClone(v.items[0].consumers[0].deliveries[0])),
    v => v.items[0].consumers[0].deliveries[0].outcome = 'unavailable']) {
    const value = structuredClone(page); change(value);
    assert.throws(() => assertRetainedRelay(ctx, value, [{event, input}]));
  }
});

test('an unknown first callback is sent once and retains fixture cleanup without replay', async () => {
  const lease = fixture(), cleanups = []; let sends = 0, replays = 0, closed = 0;
  lease.replicas = ['a', 'b'].map(slot => ({slot, instance: 'instance-' + slot, ready: true,
    image: 'eg@sha256:' + 'b'.repeat(64)}));
  lease.close = async () => closed++;
  lease.request = async ({path, replica}) => {
    const receipt = ingress().receipt;
    receipt.replica = replica; receipt.method = 'GET'; receipt.status = 200;
    receipt.request_id = path + replica;
    for (const name of ['serving', 'serving_after']) {
      Object.assign(receipt[name], {replica, instance_id: 'instance-' + replica});
    }
    return {status: 200, instance: 'instance-' + replica, requestId: receipt.request_id, eg_receipt: receipt,
      body: path.endsWith('/activity') ? {items: [], cursor: 'empty', retention_days: 7, limit: 100, has_more: false} : record()};
  };
  lease.counters = async () => ({turns: 0, effects: 0, deliveries: 0, pending: 0, unknown: 0, lateEffects: 0});
  lease.platformSend = async () => {sends++; throw Error('relay_transport_unknown');};
  lease.replayEvent = async () => {replays++; throw Error('unexpected_replay');};
  const context = {...ctx, row: {id: 'EG-035', case_id: 'C275:EG-035'}, am: 'a',
    step: async (_name, fn) => fn(), observe: async (_name, fn) => fn(), defer: (_name, fn) => cleanups.push(fn),
    fixture: name => {assert.equal(name, 'user-fixtures'); return {take: async () => lease};}};
  await assert.rejects(relayVerificationCase.run(context), /relay_transport_unknown/);
  for (const cleanup of cleanups.reverse()) await cleanup();
  assert.equal(sends, 1); assert.equal(replays, 0); assert.equal(closed, 1);
});
