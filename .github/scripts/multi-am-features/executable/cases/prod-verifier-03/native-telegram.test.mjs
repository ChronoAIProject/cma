import test from 'node:test';
import assert from 'node:assert/strict';
import {Blocked} from '../../runner.mjs';
import {digest} from './helpers.mjs';
import {setups} from './setup.mjs';
import {nativeTelegramCase, validateNativeTelegramFixture, assertNativeTelegramSource,
  assertNativeTelegramRegistration, assertNativeTelegramWire, assertNativeTelegramAssociation,
  assertNativeTelegramDelivery, assertNativeTelegramActivity} from './native-telegram.mjs';

const ctx = {assert: (value, code) => assert.ok(value, code)};
function fixture() {
  return {local: true, synthetic: true, recordId: 'record', channelId: 'channel',
    replicas: ['a', 'b'].map(slot => ({slot, instance: 'instance-' + slot, ready: true, image: 'eg@sha256:' + 'b'.repeat(64)})),
    expected: {ownerSubject: 'owner', consumerId: 'default', botId: '100', botUsername: 'synthetic_bot',
      allowedSender: '101', deniedSender: '102', privateChat: '101', groupChat: '-103',
      callbackUrl: 'https://gateway.example/v1/telegram/channel', webhookSecretSha256: digest('synthetic-secret-hash')},
    ...Object.fromEntries(['request', 'platformConfiguration', 'platformSend', 'callbackCapture', 'nativeEventCapture',
      'waitForTurn', 'providerContext', 'waitForDelivery', 'counters', 'drain', 'close'].map(k => [k, async () => {}]))};
}
function record() {
  const e = fixture().expected;
  return {record_id: 'record', channel_id: 'channel', provider: {slug: 'cma_codex'},
    endpoints: {telegram_webhook_url: e.callbackUrl}, definition: {owner: {subject: e.ownerSubject},
      sources: [{type: 'telegram', bot_id: e.botId, bot_username: e.botUsername, admission: {type: 'scoped',
        senders: {type: 'allowlist', ids: [e.allowedSender]}, chats: {type: 'allowlist', ids: [e.privateChat, e.groupChat]},
        groups: 'mention'}}], partition: {version: 1, key: 'conversation_and_sender'},
      reply: {auto_final: true, ack_after_ms: 0, sinks: [{type: 'source'}]}}};
}
function registration() {
  const e = fixture().expected;
  return {schema: 'multi-eg-telegram-registration-v1', calls: [
    {method: 'getMe', response: {ok: true, result: {id: e.botId, username: e.botUsername, is_bot: true}}},
    {method: 'setWebhook', request: {url: e.callbackUrl, secret_sha256: e.webhookSecretSha256,
      allowed_updates: ['message', 'edited_message', 'channel_post', 'edited_channel_post',
        'callback_query', 'message_reaction', 'message_reaction_count'], drop_pending_updates: false},
    response: {ok: true, result: true}},
    {method: 'getWebhookInfo', response: {ok: true, result: {url: e.callbackUrl, pending_update_count: 0}}},
  ].map((v, i) => ({...v, request_id: 'registration-' + i, status: 200, bot_id: e.botId, artifact_sha256: digest(i.toString())}))};
}
const input = {updateId: '201', messageId: '202', senderId: '101', chatId: '101',
  isGroup: false, mentioned: false, text: 'Synthetic native message'};
const event = {eventId: 'telegram:100:201', conversationId: 'conversation', responseId: 'response',
  consumerId: 'default', agentId: 'agent', eventRef: 'synthetic-opaque-reference',
  destination: '101', expectedSender: '100', expectedContentDigest: digest('Synthetic native answer')};
const ingress = {receipt: {request_id: 'ingress'}};
function wire() {
  const {text, isGroup, ...facts} = input;
  return {schema: 'multi-eg-telegram-wire-v1', request_id: 'ingress', method: 'POST', path: '/v1/telegram/channel',
    artifact_sha256: digest('wire'), body_sha256: digest('body'), secret_header_sha256: fixture().expected.webhookSecretSha256,
    facts: {...facts, chatType: 'private', isBotSender: false, textDigest: digest(text)}};
}
function original() {
  return {schema: 'multi-eg-native-event-v1', ...event, request_id: 'am-request',
    event_ref_sha256: digest(event.eventRef), seal_verified: true, artifact_sha256: digest('sealed-capture'),
    reply: {type: 'telegram', bot_id: '100', chat_id: '101', message_id: '202', thread_id: null},
    source: {type: 'telegram', platform: 'telegram', bot_id: '100'}, actor_id: '101', text_sha256: digest(input.text)};
}
function delivery() {
  return {eventId: event.eventId, responseId: event.responseId, destination: '101', sender: '100',
    contentDigest: event.expectedContentDigest, status: 'delivered', count: 1,
    platformReceipt: {schema: 'multi-eg-telegram-delivery-v1', request_id: 'send-message', artifact_sha256: digest('send'),
      method: 'sendMessage', bot_id: '100', status: 200, request: {chat_id: '101', text_sha256: event.expectedContentDigest},
      response: {ok: true, result: {chat_id: '101', message_id: '203'}}}};
}
function activity() {
  const at = '2026-11-17T00:00:00Z';
  return {cursor: 'cursor', retention_days: 7, limit: 100, has_more: false, items: [{event_id: event.eventId,
    source: original().source, actor: {id: '101'}, conversation: {id: '101', kind: 'private'},
    received_at: at, occurred_at: at, consumers: [{consumer_id: 'default', agent_id: 'agent', response_id: 'response',
      conversation_id: 'provider-conversation', state: 'completed', dispatched_at: at, accepted_at: at, produced_at: at,
      completed_at: at, deliveries: [{step: 'answer', outcome: 'confirmed', observed_at: at, confirmed_at: at}]}]}]};
}

test('native Telegram keeps exact steps and blocks incomplete setup with cleanup custody', async () => {
  assert.equal(nativeTelegramCase.id, 'C275:EG-039');
  assert.deepEqual(nativeTelegramCase.steps, ['step_1', 'step_2', 'expected_result']);
  assert.deepEqual(nativeTelegramCase.fixtures, ['user-fixtures']);
  validateNativeTelegramFixture(fixture());
  for (const method of ['nativeEventCapture', 'callbackCapture', 'platformConfiguration', 'providerContext']) {
    const lease = fixture(), cleanups = []; let closed = 0;
    delete lease[method]; lease.close = async () => closed++;
    await assert.rejects(setups.native_telegram({...ctx, row: {case_id: 'C275:EG-039'},
      defer: (_name, fn) => cleanups.push(fn)}, {eg: {acquire: async () => lease}}), error => error instanceof Blocked);
    await cleanups[0](); await cleanups[0](); assert.equal(closed, 1);
  }
});

test('native source must keep bounded sender/chat policy and distinct transport', () => {
  assertNativeTelegramSource(ctx, fixture(), record());
  for (const change of [r => r.definition.sources[0].type = 'nyxid_relay',
    r => r.definition.sources[0].bot_id = '102', r => r.endpoints.telegram_webhook_url += '/wrong',
    r => r.definition.sources[0].admission.senders = {type: 'open'},
    r => r.definition.sources[0].admission.groups = 'all',
    r => r.definition.sources[0].message_events = true, r => r.definition.reply.ack_after_ms = 1000]) {
    const value = record(); change(value);
    assert.throws(() => assertNativeTelegramSource(ctx, fixture(), value), /native_telegram_/);
  }
});

test('activation needs actual bot identity, registered secret digest and readback', () => {
  assertNativeTelegramRegistration(ctx, fixture(), registration());
  for (const change of [v => v.calls.pop(), v => v.calls[0].response.result.id = 'foreign',
    v => v.calls[1].request.secret_sha256 = digest('wrong'), v => v.calls[1].response.result = false,
    v => v.calls[1].request.drop_pending_updates = true, v => v.calls[1].request.allowed_updates = ['message'],
    v => v.calls[2].response.result.url += '/wrong', v => v.calls[2].request_id = v.calls[1].request_id]) {
    const value = registration(); change(value);
    assert.throws(() => assertNativeTelegramRegistration(ctx, fixture(), value), /native_telegram_/);
  }
});

test('native callback evidence binds exact raw message and actual authentication variant', () => {
  assertNativeTelegramWire(ctx, fixture(), wire(), ingress, input);
  const invalid = wire(); invalid.secret_header_sha256 = digest('invalid');
  assertNativeTelegramWire(ctx, fixture(), invalid, ingress, {...input, variant: 'invalid_webhook_secret'});
  assert.throws(() => assertNativeTelegramWire(ctx, fixture(), wire(), ingress,
    {...input, variant: 'invalid_webhook_secret'}), /actual_secret_variant/);
  for (const change of [v => v.request_id = 'other', v => v.path = '/v1/callbacks/channel',
    v => v.facts.messageId = '203', v => v.facts.mentioned = true, v => v.facts.textDigest = digest('other')]) {
    const value = wire(); change(value);
    assert.throws(() => assertNativeTelegramWire(ctx, fixture(), value, ingress, input), /native_telegram_/);
  }
});

test('original message association requires verified captured reference joined to actual AM request', () => {
  const turn = {amRequests: [{request_id: 'am-request'}]};
  assertNativeTelegramAssociation(ctx, fixture(), original(), event, input, turn);
  for (const change of [v => v.seal_verified = false, v => v.request_id = 'other-request',
    v => v.event_ref_sha256 = digest('other-reference'), v => v.reply.message_id = '203',
    v => v.reply.chat_id = '-103', v => v.source.type = 'nyxid_relay',
    v => v.responseId = 'other-response', v => v.actor_id = '102']) {
    const value = original(); change(value);
    assert.throws(() => assertNativeTelegramAssociation(ctx, fixture(), value, event, input, turn), /native_telegram_/);
  }
});

test('native delivery checks actual platform request without fabricating a Telegram quote', () => {
  assertNativeTelegramDelivery(ctx, fixture(), delivery(), event);
  for (const change of [v => v.replyToMessageId = '202', v => v.platformReceipt.request.chat_id = '-103',
    v => v.platformReceipt.bot_id = '102', v => v.platformReceipt.response.result.chat_id = '-103',
    v => v.platformReceipt.request.text_sha256 = digest('wrong'), v => v.platformReceipt.response.ok = false,
    v => v.platformReceipt.request.reply_to_message_id = '202', v => v.count = 2]) {
    const value = delivery(); change(value);
    assert.throws(() => assertNativeTelegramDelivery(ctx, fixture(), value, event), /native_telegram_|reply_/);
  }
});

test('final Activity must preserve native source, exact consumer and one completed answer', () => {
  assertNativeTelegramActivity(ctx, fixture(), activity(), [{event, input}]);
  for (const change of [v => v.items[0].source.type = 'nyxid_relay', v => v.items[0].actor.id = '102',
    v => v.items[0].conversation.id = '-103', v => v.items[0].consumers[0].response_id = 'other',
    v => v.items[0].consumers[0].state = 'accepted', v => v.items[0].consumers[0].deliveries[0].outcome = 'unknown',
    v => v.items.push(structuredClone(v.items[0]))]) {
    const value = activity(); change(value);
    assert.throws(() => assertNativeTelegramActivity(ctx, fixture(), value, [{event, input}]), /native_telegram_|activity_/);
  }
});

test('an unknown native callback is not replayed and cleanup retains the original failure', async () => {
  const lease = fixture(), cleanups = []; let sends = 0, closed = 0;
  lease.close = async () => {closed++;};
  lease.request = async ({path, replica}) => {
    const serving = {schema: 'c275-eg-serving-v1', scope: 'eg', replica, origin: 'http://127.0.0.1:12345',
      ready: true, pod_uid: 'pod-' + replica, instance_id: 'instance-' + replica, container_id: 'container-' + replica,
      source: 'a'.repeat(40), image: lease.replicas[0].image, observed_at: '2026-11-17T00:00:00.000Z'};
    const receipt = {scope: 'eg', replica, request_id: path + replica, method: 'GET', status: 200, serving,
      serving_after: {...serving, observed_at: '2026-11-17T00:00:00.003Z'},
      started_at: '2026-11-17T00:00:00.001Z', ended_at: '2026-11-17T00:00:00.002Z',
      attribution: {verdict: 'PASS', basis: 'pod_forward_before_after'}};
    return {status: 200, instance: serving.instance_id, requestId: receipt.request_id, eg_receipt: receipt,
      body: path.endsWith('/activity') ? {...activity(), items: []} : record()};
  };
  lease.platformConfiguration = async () => registration();
  lease.counters = async () => ({turns: 0, effects: 0, deliveries: 0, pending: 0, unknown: 0, lateEffects: 0});
  lease.platformSend = async () => {sends++; throw Error('native_transport_unknown');};
  await assert.rejects(nativeTelegramCase.run({...ctx, row: {id: 'EG-039', case_id: 'C275:EG-039'}, am: 'a',
    step: async (_name, fn) => fn(), observe: async (_name, fn) => fn(), defer: (_name, fn) => cleanups.push(fn),
    fixture: name => {assert.equal(name, 'user-fixtures'); return {take: async () => lease};}}), /native_transport_unknown/);
  for (const fn of cleanups.reverse()) await fn();
  assert.equal(sends, 1); assert.equal(closed, 1);
});
