import {randomUUID} from 'node:crypto';
import {Blocked} from '../../runner.mjs';
import {define, acquire, invoke, egRequest, peer, segment, same, digest, retainTurn, delivered} from './helpers.mjs';
import {assertRelayResponse, assertRelayCounters} from './relay-verification.mjs';

const text = value => typeof value === 'string' && value.length > 0;
const hash = value => /^[a-f0-9]{64}$/.test(value ?? '');
const markerPattern = /EGRELAY_[a-f0-9]{32}/g;
const platforms = ['telegram', 'discord', 'whatsapp', 'lark', 'feishu', 'slack'];
const native = ['telegram', 'lark', 'feishu'];

export function validateJourneyFixture(lease, platform, followup) {
  const e = lease.expected;
  if (!platforms.includes(platform) || !text(lease.recordId) || !text(lease.channelId)
    || !e || !['ownerSubject', 'issuer', 'keyId', 'routeId', 'replySender', 'consumerId', 'callbackUrl'].every(k => text(e[k]))
    || !Array.isArray(e.conversations) || e.conversations.length !== (followup ? 3 : 1)
    || !e.conversations.every(c => text(c?.senderId) && text(c.chatId)
      && (platform !== 'telegram' || c.chatId === c.senderId))
    || new Set(e.conversations.map(c => c.chatId)).size !== e.conversations.length
    || !(native.includes(platform) ? e.verificationMode === 'native_event'
      : platform === 'slack' ? e.verificationMode === 'trusted_normalized'
        : ['raw_event', 'trusted_normalized'].includes(e.verificationMode)))
    throw new Blocked('relay_journey_exact_platform_seed_required');
  for (const method of ['request', 'platformSend', 'callbackCapture', 'waitForTurn', 'providerContext',
    'waitForDelivery', 'counters', 'drain', ...(followup ? ['externalPlatformEvidence'] : [])])
    if (typeof lease[method] !== 'function') throw new Blocked('relay_journey_primitive_' + method.toLowerCase());
  return e;
}

export function assertJourneySource(ctx, lease, platform, record, capabilities) {
  const e = lease.expected, d = record?.definition, s = d?.sources?.[0];
  ctx.assert(record?.record_id === lease.recordId && record.channel_id === lease.channelId
    && record.endpoints?.nyxid_callback_url === e.callbackUrl
    && d?.owner?.subject === e.ownerSubject && record.provider?.slug === 'cma_codex'
    && d.sources?.length === 1 && s.type === 'nyxid_relay' && s.platform === platform
    && s.issuer === e.issuer && s.key_id === e.keyId && same(s.route_ids, [e.routeId]),
  'relay_journey_actual_saved_source_identity');
  const mode = s.trust_normalized === true ? 'trusted_normalized'
    : native.includes(platform) ? 'native_event' : s.verify_raw_event === true ? 'raw_event' : null;
  ctx.assert(mode === e.verificationMode
    && !(s.trust_normalized === true && s.verify_raw_event === true), 'relay_journey_saved_trust_mode');
  const advertised = capabilities?.relay_authoring?.platforms?.filter(p => p.id === platform);
  ctx.assert(advertised?.length === 1 && Array.isArray(advertised[0].verification_modes)
    && new Set(advertised[0].verification_modes.map(m => m.id)).size === advertised[0].verification_modes.length
    && advertised[0].verification_modes.every(m => text(m.id) && typeof m.writes_enabled === 'boolean'),
  'relay_journey_platform_modes_unambiguous');
  const selected = advertised[0].verification_modes.find(m => m.id === mode);
  if (selected?.writes_enabled !== true) throw new Blocked('relay_journey_selected_verification_mode_unavailable');
  ctx.assert(d.partition?.version === 1 && ['conversation', 'conversation_and_sender'].includes(d.partition.key)
    && same(d.reply, {auto_final: true, ack_after_ms: 0, sinks: [{type: 'source'}]})
    && d.routing === undefined && d.provider?.target?.type !== 'existing_agent'
    && d.profile?.environment?.session_affinity !== 'source_user', 'relay_journey_single_context_and_answer');
  return advertised[0];
}

export function assertJourneyCapture(ctx, capture, event, input, lease) {
  const normalized = lease.expected.verificationMode === 'trusted_normalized';
  ctx.assert(capture?.schema === 'multi-eg-relay-wire-v1' && capture.request_id === event.ingress.receipt.request_id
    && capture.method === 'POST' && capture.path === '/v1/callbacks/' + segment(lease.channelId)
    && ['artifact_sha256', 'body_sha256', 'signed_headers_sha256', 'callback_token_sha256', 'reply_token_sha256']
      .every(k => hash(capture[k])) && capture.callback_body_sha256 === capture.body_sha256,
  'relay_journey_independent_signed_wire_capture');
  ctx.assert(capture.factsBasis === (normalized ? 'signed_normalized' : 'raw_event')
    && same(capture.facts, {platform: input.platform, issuer: lease.expected.issuer, keyId: lease.expected.keyId,
      messageId: input.messageId, correlationId: input.correlationId, routeId: input.routeId,
      senderId: input.senderId, chatId: input.chatId, isGroup: false,
      mentioned: normalized ? null : false, isBotSender: normalized ? null : false, textDigest: digest(input.text)}),
  'relay_journey_exact_wire_facts_and_trust_basis');
}

// Inspect the actual fake-model request after the real AM has assembled its
// conversation. A delivery adapter's contextEventIds cannot prove this history.
export function assertJourneyContext(ctx, event, context, currentText, expectedMarkers) {
  ctx.assert(context?.eventId === event.eventId && context.responseId === event.responseId
    && context.agentId === event.agentId && context.conversationId === event.conversationId
    && context.requestCount === 1 && Array.isArray(context.messages) && context.messages.length > 0,
  'relay_journey_independent_provider_request');
  const userTexts = context.messages.filter(message => message.role === 'user').flatMap(message => {
    if (typeof message.content === 'string') return [message.content];
    ctx.assert(Array.isArray(message.content) && message.content.every(part =>
      ['text', 'input_text', 'output_text'].includes(part.type) && typeof part.text === 'string'),
    'relay_journey_text_provider_input');
    return message.content.map(part => part.text);
  });
  ctx.assert(userTexts.some(value => value.includes(currentText)), 'relay_journey_current_input_in_provider');
  const found = [...new Set(userTexts.flatMap(value => [...value.matchAll(markerPattern)].map(m => m[0])))].sort();
  ctx.assert(same(found, [...new Set(expectedMarkers)].sort()), 'relay_journey_complete_isolated_context');
}

export function assertJourneyContinuity(ctx, turns) {
  ctx.assert(turns.length === 6 && new Set(turns.map(t => t.event.eventId)).size === 6
    && new Set(turns.map(t => t.event.responseId)).size === 6, 'relay_journey_six_distinct_turns');
  for (let i = 0; i < turns.length; i += 2) {
    const first = turns[i].event, next = turns[i + 1].event;
    ctx.assert(next.conversationId === first.conversationId && next.agentId === first.agentId
      && next.consumerId === first.consumerId, 'relay_journey_retained_followup_conversation_agent');
  }
  ctx.assert(new Set(turns.map(t => t.event.conversationId)).size === 3
    && new Set(turns.map(t => t.event.agentId)).size === 3, 'relay_journey_three_isolated_contexts');
  ctx.assert(new Set(turns.map(t => t.evidence.servingAm)).size === 2,
    'relay_journey_both_actual_am_ingresses');
  const receipts = turns.map(t => t.event.ingress.receipt);
  ctx.assert(new Set(receipts.map(r => r.replica)).size === 2
    && new Set(receipts.map(r => r.serving.instance_id)).size === 2
    && new Set(receipts.map(r => r.request_id)).size === 6, 'relay_journey_both_actual_eg_ingresses');
}

export function relayJourney(number, platform, followup) {
  return define(number, 'eg', ['user-fixtures'], async ctx => {
    let lease, expected, initialRecord, initialCapabilities;
    const turns = [], requestIds = new Set();
    const bounded = (method, input = {}) => {
      ctx.assert(!ctx.signal?.aborted, 'relay_journey_cancelled');
      return invoke(lease, method, {...input, signal: ctx.signal,
        timeoutMs: Math.max(1, Math.min(15000, ctx.remainingMs?.() ?? 15000))});
    };
    const send = async (conversation, text, completionText, markers) => {
      const input = {platform, trustMode: 'nyxid_relay', messageId: randomUUID(), correlationId: randomUUID(),
        senderId: conversation.senderId, chatId: conversation.chatId, routeId: expected.routeId,
        isGroup: false, mentioned: false, isBotSender: false, text, completionText,
        replica: turns.length % 2 ? peer(ctx.am) : ctx.am};
      const event = await bounded('platformSend', input);
      ctx.assert(event.eventId === input.messageId && event.consumerId === expected.consumerId
        && event.platform === platform && event.trustMode === 'nyxid_relay'
        && event.destination === conversation.chatId && event.expectedSender === expected.replySender
        && event.expectedContentDigest === digest(completionText), 'relay_journey_intended_sender_destination_answer');
      assertRelayResponse(ctx, event.ingress, input.replica, event);
      ctx.assert(!requestIds.has(event.ingress.receipt.request_id), 'relay_journey_unique_callback_request');
      requestIds.add(event.ingress.receipt.request_id);
      await ctx.observe('relay_journey_ingress', async () => ({schema: 'multi-eg-request-v1', receipt: event.ingress.receipt}));
      const capture = await bounded('callbackCapture', {requestId: event.ingress.receipt.request_id});
      assertJourneyCapture(ctx, capture, event, input, lease);
      const evidence = await bounded('waitForTurn', {eventId: event.eventId});
      await retainTurn(ctx, event, evidence);
      const context = await bounded('providerContext', {eventId: event.eventId});
      assertJourneyContext(ctx, event, context, text, markers);
      const receipt = await bounded('waitForDelivery', {eventId: event.eventId});
      delivered(ctx, event, receipt);
      const turn = {event, evidence, receipt}; turns.push(turn);
      await ctx.observe('platform_event_receipt', async () => ({event_sha256: digest(event.eventId),
        conversation_sha256: digest(event.conversationId), response_sha256: digest(event.responseId), platform,
        trust_mode: 'nyxid_relay', verification_mode: expected.verificationMode,
        serving_am: evidence.servingAm, owning_am: evidence.owningAm, epoch: evidence.epoch,
        body_sha256: capture.body_sha256, capture_sha256: capture.artifact_sha256,
        provider_context_sha256: digest(context), delivery_sha256: digest(receipt), delivery_count: receipt.count}));
      // Actual third-party-platform acceptance remains a separate mandatory
      // gate for the three frozen rows. Local synthetic delivery cannot supply it.
      if (followup) await ctx.observe('external_platform_proof', () => bounded('externalPlatformEvidence', {
        eventId: event.eventId, conversationId: event.conversationId, responseId: event.responseId, platform}));
      assertRelayCounters(ctx, await bounded('drain', {eventIds: turns.map(t => t.event.eventId)}), turns.length, turns.length);
    };
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, 'relay_' + platform); expected = validateJourneyFixture(lease, platform, followup);
      for (const replica of [ctx.am, peer(ctx.am)]) {
        const capabilities = await egRequest(ctx, lease, {path: '/v1/channel-capabilities', replica});
        const record = await egRequest(ctx, lease, {path: '/v1/channels/' + segment(lease.channelId), replica});
        assertJourneySource(ctx, lease, platform, record, capabilities);
        if (initialRecord) ctx.assert(same(record, initialRecord) && same(capabilities, initialCapabilities),
          'relay_journey_both_eg_same_source_capabilities');
        else {initialRecord = record; initialCapabilities = capabilities;}
      }
      assertRelayCounters(ctx, await bounded('counters'), 0, 0);
    });
    await ctx.step('step_2', async () => {
      for (let n = 1; n <= (followup ? 3 : 1); n++) {
        const marker = 'EGRELAY_' + randomUUID().replaceAll('-', ''), conversation = expected.conversations[n - 1];
        await send(conversation, `Remember synthetic number ${n} with token ${marker}.`,
          `Acknowledged number ${n}: ${marker}`, [marker]);
        if (followup) await send(conversation, 'Repeat the most recent synthetic number and its token from our conversation.',
          `Number ${n}: ${marker}`, [marker]);
      }
      ctx.assert(turns.length === (followup ? 6 : 1), 'relay_journey_completed_turn_count');
    });
    if (followup) await ctx.step('step_3', async () => assertJourneyContinuity(ctx, turns));
    await ctx.step('expected_result', async () => {
      assertRelayCounters(ctx, await bounded('drain', {eventIds: turns.map(t => t.event.eventId)}), turns.length, turns.length);
      for (const replica of [ctx.am, peer(ctx.am)]) {
        const capabilities = await egRequest(ctx, lease, {path: '/v1/channel-capabilities', replica});
        const record = await egRequest(ctx, lease, {path: '/v1/channels/' + segment(lease.channelId), replica});
        ctx.assert(same(record, initialRecord) && same(capabilities, initialCapabilities),
          'relay_journey_preserves_configuration_and_unavailable_modes');
      }
      for (const turn of turns) delivered(ctx, turn.event, turn.receipt);
    });
  });
}
