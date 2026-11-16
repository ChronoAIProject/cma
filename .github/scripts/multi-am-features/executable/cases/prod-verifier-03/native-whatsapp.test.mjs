import test from 'node:test';
import assert from 'node:assert/strict';
import {Blocked} from '../../runner.mjs';
import {digest} from './helpers.mjs';
import {setups} from './setup.mjs';
import {nativeWhatsappCase, validateNativeWhatsappFixture, whatsappEventId, assertWhatsappSource,
  assertWhatsappCustody, assertWhatsappChallenge, assertWhatsappIngress, assertWhatsappWire,
  assertWhatsappResult, assertWhatsappAssociation, assertWhatsappDelivery, assertWhatsappActivity} from './native-whatsapp.mjs';

const ctx = {assert: (value, code) => assert.ok(value, code)};
const e = {ownerSubject: 'owner', consumerId: 'default', appId: '100', accountId: '200', phoneId: '300',
  foreignAccountId: '201', foreignPhoneId: '301', allowedSender: '400', deniedSender: '401',
  callbackUrl: 'https://gateway.example/v1/whatsapp/channel',
  verifyTokenSha256: digest('synthetic-verification-key-digest'), appSecretSha256: digest('synthetic-app-key-digest')};
function fixture() {
  return {local: true, synthetic: true, recordId: 'record', channelId: 'channel', expected: {...e},
    replicas: ['a', 'b'].map(slot => ({slot, instance: 'eg-' + slot, ready: true, image: 'eg@sha256:' + 'b'.repeat(64)})),
    ...Object.fromEntries(['request', 'platformConfiguration', 'webhookChallenge', 'platformSend', 'callbackCapture',
      'nativeEventCapture', 'waitForTurn', 'providerContext', 'waitForDelivery', 'captureEffects', 'counters', 'drain', 'close']
      .map(key => [key, async () => {}]))};
}
const source = {type: 'whatsapp', app_id: '100', waba_id: '200', phone_number_id: '300'};
function record() {
  return {record_id: 'record', channel_id: 'channel', provider: {slug: 'cma_codex'},
    endpoints: {whatsapp_webhook_url: e.callbackUrl}, definition: {owner: {subject: e.ownerSubject},
      sources: [{...source, admission: {type: 'scoped', senders: {type: 'allowlist', ids: ['400']},
        chats: {type: 'allowlist', ids: ['400']}, groups: 'deny'}}],
      partition: {version: 1, key: 'conversation_and_sender'},
      reply: {auto_final: true, ack_after_ms: 0, sinks: [{type: 'source'}]},
      routing: {version: 1, default_consumer: {name: 'default'}, default_targets: ['default'], reply_label: 'auto'}}};
}
function custody() {
  return {schema: 'multi-eg-whatsapp-custody-v1', calls: [
    {path: '/v25.0/debug_token', response: {data: {is_valid: true, app_id: '100',
      scopes: ['whatsapp_business_management', 'whatsapp_business_messaging']}}},
    {path: '/v25.0/200/phone_numbers', query: {fields: 'id', limit: '100'}, response: {data: [{id: '300'}]}},
  ].map((v, i) => ({...v, method: 'GET', status: 200, request_id: 'custody-' + i, artifact_sha256: digest('custody-' + i)}))};
}
function ingress(replica = 'a', method = 'POST', status = 200, body = {accepted: 1, refused: 0}) {
  const serving = {schema: 'c275-eg-serving-v1', scope: 'eg', replica, origin: 'http://127.0.0.1:12345',
    ready: true, pod_uid: 'pod-' + replica, instance_id: 'eg-' + replica, container_id: 'container-' + replica,
    source: 'a'.repeat(40), image: fixture().replicas[0].image, observed_at: '2026-11-17T00:00:00.000Z'};
  return {status, body, protocolVersion: '1', contentType: status === 401 ? 'application/problem+json' : 'application/json',
    receipt: {scope: 'eg', replica, request_id: 'request', method, status, serving,
      serving_after: {...serving, observed_at: '2026-11-17T00:00:00.003Z'},
      started_at: '2026-11-17T00:00:00.001Z', ended_at: '2026-11-17T00:00:00.002Z',
      attribution: {verdict: 'PASS', basis: 'pod_forward_before_after'}}};
}
const input = {accountId: '200', phoneId: '300', senderId: '400', messageId: 'wamid.synthetic',
  timestamp: 1800000000, text: 'Synthetic private WhatsApp message'};
const event = {eventId: whatsappEventId(e, input.messageId), conversationId: 'conversation', responseId: 'response',
  consumerId: 'default', agentId: 'agent', eventRef: 'synthetic-opaque-reference', destination: '400', expectedSender: '300',
  expectedContentDigest: digest('Synthetic private answer')};
const turn = {amRequests: [{request_id: 'am-request'}]};
function wire() {
  const {text, ...facts} = input;
  return {schema: 'multi-eg-whatsapp-wire-v1', request_id: 'request', method: 'POST', path: '/v1/whatsapp/channel',
    artifact_sha256: digest('wire'), body_sha256: digest('body'), signature_sha256: digest('signature'),
    expected_signature_sha256: digest('signature'), signing_key_sha256: e.appSecretSha256,
    facts: {...facts, kind: 'text', groupId: null, textDigest: digest(text)}};
}
function original() {
  return {schema: 'multi-eg-native-event-v1', ...event, request_id: 'am-request', event_ref_sha256: digest(event.eventRef),
    seal_verified: true, artifact_sha256: digest('native-event'), source: {...source}, actor_id: '400', actor_kind: 'unknown',
    text_sha256: digest(input.text), reply: {...source, recipient_id: '400', message_id: input.messageId}};
}
function delivery() {
  return {eventId: event.eventId, responseId: event.responseId, destination: '400', sender: '300',
    contentDigest: event.expectedContentDigest, status: 'delivered', count: 1,
    platformReceipt: {schema: 'multi-eg-whatsapp-delivery-v1', request_id: 'graph-send', artifact_sha256: digest('graph'),
      method: 'POST', path: '/v25.0/300/messages', status: 200,
      request: {messaging_product: 'whatsapp', recipient_type: 'individual', to: '400', context: {message_id: input.messageId},
        type: 'text', text: {preview_url: false, body_sha256: event.expectedContentDigest}},
      response: {messaging_product: 'whatsapp', contacts: [{wa_id: '400'}], messages: [{id: 'wamid.answer'}]}}};
}
function activity() {
  const at = '2026-11-17T00:00:00Z';
  return {cursor: 'cursor', retention_days: 7, limit: 100, has_more: false, items: [{event_id: event.eventId,
    source: {...source}, actor: {id: '400', kind: 'unknown'}, conversation: {id: '400', kind: 'private'},
    received_at: at, occurred_at: at, consumers: [{consumer_id: 'default', agent_id: 'agent', response_id: 'response',
      conversation_id: 'provider-conversation', state: 'completed', dispatched_at: at, accepted_at: at, produced_at: at,
      completed_at: at, deliveries: [{step: 'answer', outcome: 'confirmed', observed_at: at, confirmed_at: at}]}]}]};
}
function challenge(request) {
  const valid = request.verification === 'valid';
  const result = {ingress: ingress(request.replica, 'GET', valid ? 200 : 401,
    valid ? request.challenge : {status: 401, code: 'not_authenticated', detail: 'native_whatsapp_verify_token'})};
  if (valid) result.ingress.contentType = 'text/plain; charset=utf-8';
  const capture = {schema: 'multi-eg-whatsapp-challenge-v1', request_id: 'request', method: 'GET',
    path: '/v1/whatsapp/channel', artifact_sha256: digest('challenge'), mode: 'subscribe',
    verify_token_sha256: valid ? e.verifyTokenSha256 : digest('invalid-token'), challenge_sha256: digest(request.challenge),
    query_keys: ['hub.mode', 'hub.verify_token', 'hub.challenge']};
  return {result, capture};
}

test('native WhatsApp binds exact frozen steps and cleanup before primitive validation', async () => {
  assert.equal(nativeWhatsappCase.id, 'C275:EG-041');
  assert.deepEqual(nativeWhatsappCase.steps, ['step_1', 'step_2', 'expected_result']);
  assert.deepEqual(nativeWhatsappCase.fixtures, ['user-fixtures']);
  validateNativeWhatsappFixture(fixture());
  for (const method of ['webhookChallenge', 'callbackCapture', 'nativeEventCapture', 'captureEffects']) {
    const lease = fixture(), cleanups = []; let closed = 0;
    delete lease[method]; lease.close = async () => closed++;
    await assert.rejects(setups.native_whatsapp({...ctx, row: {case_id: 'C275:EG-041'},
      defer: (_label, fn) => cleanups.push(fn)}, {eg: {acquire: async () => lease}}), error => error instanceof Blocked);
    await cleanups[0](); await cleanups[0](); assert.equal(closed, 1);
  }
});

test('native source retains private admission and actual default-consumer durable routing', () => {
  assertWhatsappSource(ctx, fixture(), record());
  for (const change of [r => r.definition.sources[0].type = 'nyxid_relay',
    r => r.definition.sources[0].phone_number_id = '301', r => r.definition.sources[0].group_ids = ['group'],
    r => r.definition.sources[0].admission.senders = {type: 'open'}, r => delete r.definition.routing,
    r => r.definition.routing.default_targets = ['foreign'], r => r.definition.routing.reply_label = 'always',
    r => r.definition.routing.default_consumer.reply = {auto_final: false}]) {
    const value = record(); change(value);
    assert.throws(() => assertWhatsappSource(ctx, fixture(), value), /native_whatsapp_/);
  }
});

test('app and account custody require real scoped Graph reads', () => {
  assertWhatsappCustody(ctx, fixture(), custody());
  for (const change of [c => c.calls[0].response.data.app_id = '999', c => c.calls[0].response.data.scopes.pop(),
    c => c.calls[0].response.data.is_valid = false, c => c.calls[1].path = '/v25.0/201/phone_numbers',
    c => c.calls[1].response.data = [], c => c.calls[1].request_id = c.calls[0].request_id]) {
    const value = custody(); change(value);
    assert.throws(() => assertWhatsappCustody(ctx, fixture(), value), /native_whatsapp_/);
  }
});

test('challenge must echo fresh exact text and reject invalid token with typed 401', () => {
  for (const verification of ['valid', 'invalid']) {
    const request = {replica: 'a', verification, challenge: 'fresh-challenge'};
    const {result, capture} = challenge(request);
    assertWhatsappChallenge(ctx, fixture(), result, capture, request);
    assert.throws(() => assertWhatsappChallenge(ctx, fixture(), result, {...capture,
      challenge_sha256: digest('stale')}, request), /actual_challenge_query/);
    result.ingress.status = verification === 'valid' ? 202 : 403;
    result.ingress.receipt.status = result.ingress.status;
    assert.throws(() => assertWhatsappChallenge(ctx, fixture(), result, capture, request), /native_whatsapp_/);
  }
  const value = ingress(); delete value.receipt.serving_after;
  assert.throws(() => assertWhatsappIngress(ctx, value, 'a', 'POST'), /actual_ingress_receipt/);
});

test('native signed wire cannot change body identity or pretend an invalid signature matched', () => {
  assertWhatsappWire(ctx, fixture(), wire(), ingress(), input);
  for (const change of [c => c.request_id = 'foreign', c => c.facts.accountId = '201',
    c => c.facts.phoneId = '301', c => c.facts.senderId = '401', c => c.facts.groupId = 'group',
    c => c.facts.textDigest = digest('wrong'), c => c.signing_key_sha256 = digest('foreign-key')]) {
    const value = wire(); change(value);
    assert.throws(() => assertWhatsappWire(ctx, fixture(), value, ingress(), input), /native_whatsapp_/);
  }
  assert.throws(() => assertWhatsappWire(ctx, fixture(), wire(), ingress(), {...input, variant: 'invalid_signature'}),
    /independent_signed_body/);
});

test('native protocol distinguishes accepted, denied, ignored and unauthenticated callbacks', () => {
  for (const [outcome, body] of [['accepted', {accepted: 1, refused: 0}],
    ['denied_sender', {accepted: 0, refused: 1}], ['wrong_phone', {accepted: 0, refused: 0}],
    ['wrong_account', {accepted: 0, refused: 0}]]) {
    assertWhatsappResult(ctx, ingress('a', 'POST', 200, body), outcome);
    assert.throws(() => assertWhatsappResult(ctx, ingress('a', 'POST', 202, body), outcome), /exact_accepted_refused_ignored_counts/);
  }
  assert.throws(() => assertWhatsappResult(ctx, ingress('a', 'POST', 200, {accepted: 0, refused: 1}), 'wrong_phone'),
    /exact_accepted_refused_ignored_counts/);
  const failure = ingress('a', 'POST', 401, {status: 401, code: 'not_authenticated', detail: 'native_whatsapp_signature'});
  assertWhatsappResult(ctx, failure, 'invalid_signature');
  failure.body.code = 'callback_invalid';
  assert.throws(() => assertWhatsappResult(ctx, failure, 'invalid_signature'), /exact_authentication_failure/);
});

test('native provider evidence retains scoped event identity and unknown actor kind', () => {
  assertWhatsappAssociation(ctx, fixture(), original(), event, input, turn);
  assert.notEqual(whatsappEventId(e, input.messageId), whatsappEventId({...e, phoneId: '301'}, input.messageId));
  for (const change of [c => c.request_id = 'unrelated', c => c.seal_verified = false,
    c => c.actor_kind = 'human', c => c.reply.message_id = 'wamid.other', c => c.reply.phone_number_id = '301',
    c => c.source.type = 'nyxid_relay', c => c.event_ref_sha256 = digest('another-reference')]) {
    const value = original(); change(value);
    assert.throws(() => assertWhatsappAssociation(ctx, fixture(), value, event, input, turn), /native_whatsapp_/);
  }
});

test('actual Graph delivery must match phone, original message, destination and one answer', () => {
  assertWhatsappDelivery(ctx, fixture(), delivery(), event, input);
  for (const change of [r => r.platformReceipt.path = '/v25.0/301/messages',
    r => r.platformReceipt.request.context.message_id = 'wamid.other', r => r.platformReceipt.request.recipient_type = 'group',
    r => r.platformReceipt.response.contacts[0].wa_id = '401', r => r.platformReceipt.request.text.preview_url = true,
    r => r.platformReceipt.response.messages.push({id: 'wamid.extra'}), r => r.count = 2]) {
    const value = delivery(); change(value);
    assert.throws(() => assertWhatsappDelivery(ctx, fixture(), value, event, input), /native_whatsapp_|reply_/);
  }
});

test('retained Activity preserves private native source and refuses inferred human identity', () => {
  assertWhatsappActivity(ctx, fixture(), activity(), event);
  for (const change of [p => p.items[0].actor.kind = 'human', p => p.items[0].source.phone_number_id = '301',
    p => p.items[0].consumers[0].response_id = 'other', p => p.items[0].consumers[0].state = 'accepted',
    p => p.items[0].consumers[0].deliveries.push({...p.items[0].consumers[0].deliveries[0], step: 'ack'})]) {
    const value = activity(); change(value);
    assert.throws(() => assertWhatsappActivity(ctx, fixture(), value, event), /native_whatsapp_/);
  }
});

test('unknown challenge or callback is never resubmitted and keeps fixture cleanup', async () => {
  for (const fail of ['challenge', 'callback']) {
    const lease = fixture(), cleanups = [], captures = new Map(); let sequence = 0, closed = 0, challenges = 0, sends = 0;
    lease.close = async () => closed++;
    lease.request = async ({path, replica}) => {
      const receipt = ingress(replica, 'GET').receipt; receipt.request_id = String(++sequence);
      return {status: 200, instance: 'eg-' + replica, requestId: receipt.request_id, eg_receipt: receipt,
        body: path.endsWith('/activity') ? {...activity(), items: []} : record()};
    };
    lease.webhookChallenge = async request => {
      challenges++; if (fail === 'challenge') throw Error('whatsapp_challenge_unknown');
      const {result, capture} = challenge(request), id = String(++sequence);
      result.ingress.receipt.request_id = id; capture.request_id = id; captures.set(id, capture); return result;
    };
    lease.callbackCapture = async ({requestId}) => captures.get(requestId);
    lease.platformConfiguration = async () => custody();
    lease.counters = async () => ({turns: 0, effects: 0, deliveries: 0, pending: 0, unknown: 0, lateEffects: 0});
    lease.platformSend = async () => {sends++; throw Error('whatsapp_callback_unknown');};
    await assert.rejects(nativeWhatsappCase.run({...ctx, row: {id: 'EG-041', case_id: 'C275:EG-041'}, am: 'a',
      step: async (_label, fn) => fn(), observe: async (_label, fn) => fn(), defer: (_label, fn) => cleanups.push(fn),
      fixture: name => {assert.equal(name, 'user-fixtures'); return {take: async () => lease};}}), new RegExp('whatsapp_' + fail + '_unknown'));
    for (const cleanup of cleanups.reverse()) await cleanup();
    assert.equal(closed, 1); assert.equal(challenges, fail === 'challenge' ? 1 : 4);
    assert.equal(sends, fail === 'callback' ? 1 : 0);
  }
});
