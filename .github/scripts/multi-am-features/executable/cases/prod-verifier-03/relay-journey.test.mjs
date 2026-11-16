import test from 'node:test';
import assert from 'node:assert/strict';
import {Blocked} from '../../runner.mjs';
import {digest} from './helpers.mjs';
import {setups} from './setup.mjs';
import {relayJourney, validateJourneyFixture, assertJourneySource, assertJourneyCapture,
  assertJourneyContext, assertJourneyContinuity} from './relay-journey.mjs';

const ctx = {assert: (value, code) => assert.ok(value, code)};
const variants = [[32, 'telegram', true], [33, 'discord', true], [34, 'whatsapp', true],
  [36, 'lark', false], [37, 'feishu', false], [38, 'slack', false]];
function fixture(platform = 'telegram', followup = true) {
  return {local: true, synthetic: true, recordId: 'record', channelId: 'channel', replicas: [{ready: true}, {ready: true}],
    expected: {ownerSubject: 'owner', issuer: 'issuer', keyId: 'key-id', routeId: 'route', replySender: 'bot',
      consumerId: 'default', callbackUrl: 'http://127.0.0.1:12345/v1/callbacks/channel',
      verificationMode: platform === 'slack' ? 'trusted_normalized'
        : ['discord', 'whatsapp'].includes(platform) ? 'raw_event' : 'native_event',
      conversations: Array.from({length: followup ? 3 : 1}, (_, i) => ({senderId: String(101 + i), chatId: String(101 + i)}))},
    ...Object.fromEntries(['request', 'platformSend', 'callbackCapture', 'waitForTurn', 'providerContext',
      'waitForDelivery', 'counters', 'drain', 'externalPlatformEvidence', 'close'].map(key => [key, async () => {}]))};
}
function record(lease, platform) {
  const e = lease.expected;
  return {record_id: lease.recordId, channel_id: lease.channelId, provider: {slug: 'cma_codex'},
    endpoints: {nyxid_callback_url: e.callbackUrl}, definition: {owner: {subject: e.ownerSubject},
      sources: [{type: 'nyxid_relay', platform, issuer: e.issuer, key_id: e.keyId, route_ids: [e.routeId],
        ...(e.verificationMode === 'raw_event' ? {verify_raw_event: true}
          : e.verificationMode === 'trusted_normalized' ? {trust_normalized: true} : {})}],
      partition: {version: 1, key: 'conversation_and_sender'},
      reply: {auto_final: true, ack_after_ms: 0, sinks: [{type: 'source'}]}}};
}
const capabilities = (platform, mode, enabled = true) => ({relay_authoring: {platforms: [
  {id: platform, verification_modes: [{id: mode, writes_enabled: enabled}]}]}});

test('all six relay journeys preserve frozen steps and named fixture dispatch', () => {
  for (const [number, platform, followup] of variants) {
    const value = relayJourney(number, platform, followup);
    assert.equal(value.id, 'C275:EG-' + number.toString().padStart(3, '0'));
    assert.deepEqual(value.steps, ['step_1', 'step_2', ...(followup ? ['step_3'] : []), 'expected_result']);
    assert.deepEqual(value.fixtures, ['user-fixtures']);
    validateJourneyFixture(fixture(platform, followup), platform, followup);
  }
});

test('missing authentic provider history and external evidence block with cleanup custody', async () => {
  for (const method of ['providerContext', 'callbackCapture', 'externalPlatformEvidence']) {
    const lease = fixture(), cleanups = []; let closed = 0;
    delete lease[method]; lease.close = async () => closed++;
    await assert.rejects(setups.relay_telegram({...ctx, row: {case_id: 'C275:EG-032'},
      defer: (_label, fn) => cleanups.push(fn)}, {eg: {acquire: async () => lease}}), error => error instanceof Blocked);
    await cleanups[0](); await cleanups[0](); assert.equal(closed, 1);
  }
  const collapsed = fixture(); collapsed.expected.conversations[1].chatId = collapsed.expected.conversations[0].chatId;
  assert.throws(() => validateJourneyFixture(collapsed, 'telegram', true), error => error instanceof Blocked);
});

test('saved source proves exact platform, callback and trust mode without enabling unavailable modes', () => {
  for (const [, platform, followup] of variants) {
    const lease = fixture(platform, followup), value = record(lease, platform);
    const modes = capabilities(platform, lease.expected.verificationMode);
    assertJourneySource(ctx, lease, platform, value, modes);
    assert.throws(() => assertJourneySource(ctx, lease, platform, value,
      capabilities(platform, lease.expected.verificationMode, false)), error => error instanceof Blocked);
    for (const change of [v => v.endpoints.nyxid_callback_url += '/wrong',
      v => v.definition.sources[0].issuer = 'wrong-issuer',
      v => v.definition.sources[0].route_ids.push('foreign-route'),
      v => v.definition.sources[0].trust_normalized = !v.definition.sources[0].trust_normalized,
      v => v.definition.reply.ack_after_ms = 1000]) {
      const broken = structuredClone(value); change(broken);
      assert.throws(() => assertJourneySource(ctx, lease, platform, broken, modes), /relay_journey_/);
    }
  }
});

test('normalized trust never claims independently verified sender or mention facts', () => {
  const lease = fixture('slack', false), input = {platform: 'slack', messageId: 'event', correlationId: 'correlation',
    routeId: 'route', senderId: '101', chatId: '101', text: 'Synthetic message'};
  const event = {ingress: {receipt: {request_id: 'request'}}};
  const capture = {schema: 'multi-eg-relay-wire-v1', request_id: 'request', method: 'POST',
    path: '/v1/callbacks/channel', factsBasis: 'signed_normalized',
    ...Object.fromEntries(['artifact_sha256', 'body_sha256', 'signed_headers_sha256', 'callback_token_sha256',
      'reply_token_sha256', 'callback_body_sha256'].map(key => [key, digest('capture')])),
    facts: {platform: 'slack', issuer: 'issuer', keyId: 'key-id', messageId: 'event', correlationId: 'correlation',
      routeId: 'route', senderId: '101', chatId: '101', isGroup: false, mentioned: null,
      isBotSender: null, textDigest: digest(input.text)}};
  assertJourneyCapture(ctx, capture, event, input, lease);
  for (const change of [v => v.factsBasis = 'raw_event', v => v.facts.mentioned = false,
    v => v.facts.isBotSender = false, v => v.request_id = 'other', v => v.callback_body_sha256 = digest('other')]) {
    const value = structuredClone(capture); change(value);
    assert.throws(() => assertJourneyCapture(ctx, value, event, input, lease), /relay_journey_/);
  }
});

test('follow-up history must appear in the actual provider input with no foreign context', () => {
  const marker = 'EGRELAY_' + 'a'.repeat(32), other = 'EGRELAY_' + 'b'.repeat(32);
  const event = {eventId: 'event', responseId: 'response', conversationId: 'conversation', agentId: 'agent'};
  const prompt = 'Repeat the most recent synthetic number and its token from our conversation.';
  const context = {...event, requestCount: 1, messages: [
    {role: 'user', content: [{type: 'input_text', text: 'Remember synthetic number 1 with token ' + marker}]},
    {role: 'assistant', content: 'Acknowledged ' + marker}, {role: 'user', content: prompt}]};
  assertJourneyContext(ctx, event, context, prompt, [marker]);
  for (const change of [v => v.messages.shift(), v => v.messages.pop(), v => v.responseId = 'different',
    v => v.requestCount = 2, v => v.messages.push({role: 'user', content: other})]) {
    const broken = structuredClone(context); change(broken);
    assert.throws(() => assertJourneyContext(ctx, event, broken, prompt, [marker]), /relay_journey_/);
  }
});

test('three follow-up pairs require distinct responses and actual EG/AM coverage', () => {
  const turns = Array.from({length: 6}, (_, i) => {
    const pair = Math.floor(i / 2), slot = i % 2 ? 'b' : 'a';
    return {event: {eventId: 'event-' + i, responseId: 'response-' + i, conversationId: 'conversation-' + pair,
      agentId: 'agent-' + pair, consumerId: 'default', ingress: {receipt: {replica: slot,
        serving: {instance_id: 'eg-' + slot}, request_id: 'request-' + i}}}, evidence: {servingAm: pair % 2 ? 'b' : 'a'}};
  });
  assertJourneyContinuity(ctx, turns);
  for (const change of [v => v[1].event.agentId = 'other-agent', v => v[1].event.responseId = 'response-0',
    v => v.forEach(t => t.evidence.servingAm = 'a'),
    v => v.forEach(t => t.event.ingress.receipt.serving.instance_id = 'same-process'),
    v => v[1].event.ingress.receipt.request_id = 'request-0']) {
    const broken = structuredClone(turns); change(broken);
    assert.throws(() => assertJourneyContinuity(ctx, broken), /relay_journey_/);
  }
});
