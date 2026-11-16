import {randomUUID} from 'node:crypto';
import {Blocked} from '../../runner.mjs';
import {validEgReceipt} from '../../evidence-scope.mjs';
import {define, acquire, invoke, egRequest, peer, segment, digest, same, retainTurn, delivered} from './helpers.mjs';
import {assertActivityPage} from './activity-observer.mjs';

const text = value => typeof value === 'string' && value.length > 0;
const hash = value => /^[a-f0-9]{64}$/.test(value ?? '');
const methods = ['request', 'platformSend', 'replayEvent', 'callbackCapture', 'waitForTurn',
  'releaseCompletion', 'waitForDelivery', 'counters', 'quiesce', 'drain'];

export function validateRelayFixture(lease) {
  const e = lease.expected;
  if (!text(lease.recordId) || !text(lease.channelId) || !e
    || !['issuer', 'keyId', 'ownerSubject', 'consumerId', 'replySender', 'botId', 'botUsername',
      'allowedSender', 'deniedSender', 'privateChat', 'groupChat', 'privateRoute', 'groupRoute', 'deniedRoute'].every(k => text(e[k]))
    || e.allowedSender === e.deniedSender || e.privateChat === e.groupChat
    || e.privateChat !== e.allowedSender || e.allowedSender === e.botId || e.deniedSender === e.botId
    || ![e.allowedSender, e.deniedSender, e.botId].every(value => /^[1-9][0-9]*$/.test(value))
    || !/^-[1-9][0-9]*$/.test(e.groupChat)
    || new Set([e.privateRoute, e.groupRoute, e.deniedRoute]).size !== 3)
    throw new Blocked('relay_verification_isolated_signed_fixture_required');
  for (const method of methods) if (typeof lease[method] !== 'function')
    throw new Blocked('relay_verification_primitive_' + method.toLowerCase());
  return e;
}

export function assertRelayConfiguration(ctx, lease, record) {
  const e = lease.expected, d = record?.definition, source = d?.sources?.[0];
  ctx.assert(record?.record_id === lease.recordId && record.channel_id === lease.channelId
    && d?.owner?.subject === e.ownerSubject && record.provider?.slug === 'cma_codex'
    && d.sources?.length === 1 && source?.type === 'nyxid_relay' && source.platform === 'telegram'
    && source.issuer === e.issuer && source.key_id === e.keyId && source.trust_normalized !== true
    && source.bot_id === e.botId && source.bot_username === e.botUsername
    && Array.isArray(source.route_ids)
    && same([...source.route_ids].sort(), [e.privateRoute, e.groupRoute].sort()), 'relay_exact_verified_source');
  const admission = source.admission;
  ctx.assert(admission?.type === 'scoped' && same(admission.senders, {type: 'allowlist', ids: [e.allowedSender]})
    && same(admission.chats, {type: 'allowlist', ids: [e.privateChat, e.groupChat]})
    && admission.groups === 'mention' && [undefined, 'deny'].includes(admission.bot_messages),
  'relay_exact_sender_chat_mention_policy');
  ctx.assert(same(d.partition, {version: 1, key: 'conversation_and_sender'})
    && same(d.reply, {auto_final: true, ack_after_ms: 0, sinks: [{type: 'source'}]})
    && d.routing === undefined, 'relay_isolated_partition_reply_policy');
}

export function assertRelayResponse(ctx, ingress, replica, expected) {
  ctx.assert(validEgReceipt(ingress?.receipt) && ingress.receipt.method === 'POST'
    && ingress.receipt.replica === replica && ingress.receipt.status === ingress.status
    && ingress.protocolVersion === '1', 'relay_actual_callback_receipt');
  const body = ingress.body;
  if (expected.detail) {
    ctx.assert(ingress.status === 401 && ingress.contentType?.startsWith('application/problem+json')
      && body?.status === 401 && body.code === 'callback_invalid' && body.detail === expected.detail
      && body.event_id === undefined && body.conversation_id === undefined,
    'relay_exact_unauthenticated_rejection');
  } else if (expected.reason) {
    ctx.assert(ingress.status === 202 && ingress.contentType?.startsWith('application/json')
      && same(body, {status: 'refused', reason: expected.reason}), 'relay_exact_authenticated_refusal');
  } else {
    ctx.assert(ingress.status === 202 && ingress.contentType?.startsWith('application/json')
      && same(body, {status: 'accepted', event_id: expected.eventId, conversation_id: expected.conversationId})
      && text(expected.eventId) && text(expected.conversationId), 'relay_exact_accepted_identity');
  }
}

// The parent parses its independent ingress capture and returns only bounded
// facts/digests. Callback/reply tokens and the original bytes remain in custody
// there. An expected-input echo or a generic outcome is not a capture.
export function assertRelayCapture(ctx, capture, ingress, input, lease) {
  const e = lease.expected;
  ctx.assert(capture?.schema === 'multi-eg-relay-wire-v1' && capture.request_id === ingress.receipt.request_id
    && capture.method === 'POST' && capture.path === '/v1/callbacks/' + segment(lease.channelId)
    && ['artifact_sha256', 'body_sha256', 'signed_headers_sha256', 'callback_token_sha256', 'reply_token_sha256']
      .every(k => hash(capture[k])), 'relay_independent_wire_capture');
  ctx.assert(same(capture.facts, {platform: 'telegram', issuer: e.issuer, keyId: e.keyId,
    messageId: input.messageId, correlationId: input.correlationId, routeId: input.routeId,
    chatId: input.chatId, senderId: input.senderId, isGroup: input.isGroup, mentioned: input.mentioned,
    isBotSender: false, textDigest: digest(input.text)}), 'relay_exact_wire_facts');
  ctx.assert(capture.callback_body_sha256 === capture.body_sha256, 'relay_signed_body_digest_matches_wire');
}

export function assertRelayReplay(ctx, original, replay) {
  ctx.assert(original.request_id !== replay.request_id, 'relay_replay_distinct_http_request');
  for (const field of ['body_sha256', 'signed_headers_sha256', 'callback_token_sha256', 'reply_token_sha256', 'facts'])
    ctx.assert(same(original[field], replay[field]), 'relay_replay_same_signed_' + field);
}

export function assertRelayCounters(ctx, counts, turns, deliveries, held = false) {
  ctx.assert(counts?.turns === turns && counts.effects === turns && counts.deliveries === deliveries
    && counts.pending === (held ? 1 : 0) && counts.unknown === 0 && counts.lateEffects === 0,
  'relay_independent_exact_work_counts');
}

export function assertRetainedRelay(ctx, page, events) {
  assertActivityPage(ctx, page, events.map(({event}) => event.eventId));
  for (const {event, input} of events) {
    const row = page.items.find(row => row.event_id === event.eventId);
    ctx.assert(row.actor.id === input.senderId && row.conversation.id === input.chatId
      && row.conversation.kind === (input.isGroup ? 'group' : 'private')
      && same(row.source, {type: 'nyxid_relay', platform: 'telegram', route_id: input.routeId}),
    'relay_retained_verified_sender_conversation_source');
    ctx.assert(row.consumers.length === 1, 'relay_retained_one_consumer');
    const consumer = row.consumers[0];
    ctx.assert(consumer.consumer_id === event.consumerId && consumer.agent_id === event.agentId
      && consumer.response_id === event.responseId && text(consumer.conversation_id)
      && consumer.state === 'completed' && ['dispatched_at', 'accepted_at', 'produced_at', 'completed_at']
        .every(k => Number.isFinite(Date.parse(consumer[k]))),
    'relay_retained_exact_completed_turn');
    ctx.assert(consumer.deliveries.length === 1 && consumer.deliveries[0].step === 'answer'
      && consumer.deliveries[0].outcome === 'confirmed'
      && Number.isFinite(Date.parse(consumer.deliveries[0].confirmed_at)), 'relay_retained_one_confirmed_answer');
  }
}

export const relayVerificationCase = define(35, 'eg', ['user-fixtures'], async ctx => {
  let lease, expected, original, replayed, configuration;
  const accepted = [], requestIds = new Set(), receipts = [];
  const bounded = (method, args = {}, ms = 15000) => {
    ctx.assert(!ctx.signal?.aborted, 'relay_case_cancelled');
    return invoke(lease, method, {...args, signal: ctx.signal,
      timeoutMs: Math.max(1, Math.min(ms, ctx.remainingMs?.() ?? ms))});
  };
  const input = (group = false, changes = {}) => ({platform: 'telegram', trustMode: 'nyxid_relay',
    messageId: randomUUID(), correlationId: randomUUID(), senderId: expected.allowedSender,
    chatId: group ? expected.groupChat : expected.privateChat,
    routeId: group ? expected.groupRoute : expected.privateRoute, isGroup: group, mentioned: group,
    isBotSender: false, replica: ctx.am,
    text: (group ? '@' + expected.botUsername + ' ' : '') + 'Relay verification ' + randomUUID(),
    completionText: 'Synthetic relay answer ' + randomUUID(), ...changes});
  const retainIngress = async (result, request, outcome) => {
    assertRelayResponse(ctx, result.ingress, request.replica, outcome);
    const receipt = result.ingress.receipt;
    ctx.assert(!requestIds.has(receipt.request_id), 'relay_unique_request_receipt'); requestIds.add(receipt.request_id);
    const capture = await bounded('callbackCapture', {requestId: receipt.request_id});
    assertRelayCapture(ctx, capture, result.ingress, request, lease);
    receipts.push(receipt);
    await ctx.observe('relay_ingress_receipt', async () => ({schema: 'multi-eg-request-v1', receipt}));
    await ctx.observe('relay_wire_digest', async () => ({request_sha256: digest(receipt.request_id),
      body_sha256: capture.body_sha256, signed_headers_sha256: capture.signed_headers_sha256,
      artifact_sha256: capture.artifact_sha256, response_sha256: digest(result.ingress.body)}));
    return capture;
  };
  const admit = async request => {
    const event = await bounded('platformSend', request);
    ctx.assert(event.eventId === request.messageId && event.consumerId === expected.consumerId
      && event.platform === 'telegram' && event.trustMode === 'nyxid_relay'
      && event.destination === request.chatId && event.expectedSender === expected.replySender
      && event.expectedContentDigest === digest(request.completionText), 'relay_intended_turn_destination_content');
    const capture = await retainIngress(event, request, event);
    const turn = await bounded('waitForTurn', {eventId: event.eventId, stage: 'accepted'});
    await retainTurn(ctx, event, turn);
    const result = {event, input: request, capture, turn}; accepted.push(result); return result;
  };
  const replay = async (first, replica) => {
    // This is an explicit same-wire duplicate test, never recovery by replay
    // after an unknown network outcome. One fixture invocation sends once.
    const result = await bounded('replayEvent', {eventId: first.event.eventId, replica});
    const capture = await retainIngress(result, {...first.input, replica}, first.event);
    assertRelayReplay(ctx, first.capture, capture);
    return result.ingress.receipt;
  };
  const finish = async turn => {
    await bounded('releaseCompletion', {eventId: turn.event.eventId});
    const receipt = await bounded('waitForDelivery', {eventId: turn.event.eventId});
    delivered(ctx, turn.event, receipt);
    await ctx.observe('relay_delivery', async () => ({event_sha256: digest(turn.event.eventId),
      response_sha256: digest(turn.event.responseId), delivery_sha256: digest(receipt), count: receipt.count}));
    assertRelayCounters(ctx, await bounded('drain', {eventIds: accepted.map(v => v.event.eventId)}), accepted.length, accepted.length);
  };
  const activity = async replica => {
    const response = await egRequest(ctx, lease, {path: '/v1/channels/' + segment(lease.channelId) + '/activity',
      replica, returnResponse: true});
    ctx.assert(validEgReceipt(response.eg_receipt), 'relay_actual_activity_receipt');
    assertRetainedRelay(ctx, response.body, accepted);
    return response.body;
  };
  await ctx.step('step_1', async () => {
    lease = await acquire(ctx, 'relay_verification'); expected = validateRelayFixture(lease);
    for (const replica of [ctx.am, peer(ctx.am)]) {
      const record = await egRequest(ctx, lease, {path: '/v1/channels/' + segment(lease.channelId), replica});
      assertRelayConfiguration(ctx, lease, record);
      if (configuration) ctx.assert(same(record, configuration), 'relay_both_eg_same_initial_configuration');
      else configuration = record;
      await activity(replica);
    }
    assertRelayCounters(ctx, await bounded('counters'), 0, 0);
    original = await admit(input(false, {holdCompletion: true}));
    assertRelayCounters(ctx, await bounded('counters'), 1, 0, true);
    replayed = await replay(original, peer(original.event.ingress.receipt.replica));
    ctx.assert(replayed.replica !== original.event.ingress.receipt.replica
      && replayed.serving.instance_id !== original.event.ingress.receipt.serving.instance_id,
    'relay_duplicate_reaches_distinct_real_eg');
    assertRelayCounters(ctx, await bounded('quiesce', {eventId: original.event.eventId}, 5000), 1, 0, true);
    await finish(original);
  });
  await ctx.step('step_2', async () => {
    for (const replica of [ctx.am, peer(ctx.am)]) {
      const negatives = [
        {request: input(false, {replica, variant: 'invalid_signature'}), outcome: {detail: 'token_signature'}},
        {request: input(false, {replica, routeId: expected.deniedRoute}), outcome: {detail: 'route_denied'}},
        {request: input(true, {replica, senderId: expected.deniedSender}), outcome: {reason: 'sender_denied'}},
        {request: input(true, {replica, mentioned: false, text: 'Unmentioned synthetic group ' + randomUUID()}),
          outcome: {reason: 'group_not_addressed'}},
      ];
      for (const {request, outcome} of negatives) {
        const result = await bounded('platformSend', request);
        await retainIngress(result, request, outcome);
        assertRelayCounters(ctx, await bounded('quiesce', {messageId: request.messageId}, 5000), 1, 1);
      }
    }
    const group = await admit(input(true, {replica: peer(ctx.am), holdCompletion: true}));
    ctx.assert(group.event.conversationId !== original.event.conversationId,
      'relay_private_group_context_isolation');
    assertRelayCounters(ctx, await bounded('counters'), 2, 1, true);
    await finish(group);
    const before = await activity(peer(ctx.am));
    for (const replica of [ctx.am, peer(ctx.am)]) {
      await replay(original, replica);
      assertRelayCounters(ctx, await bounded('quiesce', {eventId: original.event.eventId}, 5000), 2, 2);
      const after = await activity(replica);
      ctx.assert(same(after.items, before.items), 'relay_retained_replay_keeps_original_activity');
    }
  });
  await ctx.step('expected_result', async () => {
    assertRelayCounters(ctx, await bounded('drain', {eventIds: accepted.map(v => v.event.eventId)}), 2, 2);
    for (const replica of [ctx.am, peer(ctx.am)]) {
      await activity(replica);
      const record = await egRequest(ctx, lease, {path: '/v1/channels/' + segment(lease.channelId), replica});
      ctx.assert(same(record, configuration), 'relay_verification_preserves_channel_definition');
    }
    ctx.assert(new Set(receipts.map(r => r.replica)).size === 2
      && receipts.length === 13 && requestIds.size === 13, 'relay_all_original_duplicate_negative_receipts');
    await ctx.observe('relay_verification_counts', async () => ({accepted_events: 2, duplicate_requests: 3,
      rejected_requests: 4, refused_requests: 4, turns: 2, deliveries: 2, pending: 0, unknown: 0}));
  });
});
