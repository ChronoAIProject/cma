import {randomInt, randomUUID} from 'node:crypto';
import {Blocked} from '../../runner.mjs';
import {validEgReceipt} from '../../evidence-scope.mjs';
import {define, acquire, invoke, egRequest, peer, segment, same, digest, retainTurn, delivered} from './helpers.mjs';
import {assertRelayResponse, assertRelayCounters} from './relay-verification.mjs';
import {assertJourneyContext} from './relay-journey.mjs';
import {assertActivityPage} from './activity-observer.mjs';

const text = value => typeof value === 'string' && value.length > 0;
const hash = value => /^[a-f0-9]{64}$/.test(value ?? '');
const positive = value => typeof value === 'string' && /^[1-9][0-9]*$/.test(value);
const updates = ['message', 'edited_message', 'channel_post', 'edited_channel_post',
  'callback_query', 'message_reaction', 'message_reaction_count'];

export function validateNativeTelegramFixture(lease) {
  const e = lease.expected;
  if (!text(lease.recordId) || !text(lease.channelId) || !e
    || !['ownerSubject', 'consumerId', 'botUsername', 'callbackUrl'].every(k => text(e[k]))
    || !['botId', 'allowedSender', 'deniedSender', 'privateChat'].every(k => positive(e[k]))
    || new Set([e.botId, e.allowedSender, e.deniedSender]).size !== 3
    || e.privateChat !== e.allowedSender || !/^-[1-9][0-9]*$/.test(e.groupChat ?? '')
    || !hash(e.webhookSecretSha256)) throw new Blocked('native_telegram_isolated_seed_required');
  for (const method of ['request', 'platformConfiguration', 'platformSend', 'callbackCapture', 'nativeEventCapture',
    'waitForTurn', 'providerContext', 'waitForDelivery', 'counters', 'drain'])
    if (typeof lease[method] !== 'function') throw new Blocked('native_telegram_primitive_' + method.toLowerCase());
  return e;
}

export function assertNativeTelegramSource(ctx, lease, record) {
  const e = lease.expected, d = record?.definition, s = d?.sources?.[0];
  ctx.assert(record?.record_id === lease.recordId && record.channel_id === lease.channelId
    && record.endpoints?.telegram_webhook_url === e.callbackUrl && d?.owner?.subject === e.ownerSubject
    && record.provider?.slug === 'cma_codex' && d.sources?.length === 1
    && s.type === 'telegram' && s.bot_id === e.botId && s.bot_username === e.botUsername,
  'native_telegram_actual_source_identity');
  ctx.assert(s.admission?.type === 'scoped'
    && same(s.admission.senders, {type: 'allowlist', ids: [e.allowedSender]})
    && same(s.admission.chats, {type: 'allowlist', ids: [e.privateChat, e.groupChat]})
    && s.admission.groups === 'mention' && [undefined, 'deny'].includes(s.admission.bot_messages)
    && ['message_events', 'channel_posts', 'topic_lifecycle', 'topic_creation'].every(k => s[k] !== true),
  'native_telegram_exact_admission_scope');
  ctx.assert(same(d.partition, {version: 1, key: 'conversation_and_sender'})
    && same(d.reply, {auto_final: true, ack_after_ms: 0, sinks: [{type: 'source'}]})
    && d.routing === undefined && d.provider?.target?.type !== 'existing_agent'
    && d.profile?.environment?.session_affinity !== 'source_user', 'native_telegram_isolated_reply_policy');
}

export function assertNativeTelegramRegistration(ctx, lease, registration) {
  const e = lease.expected, calls = registration?.calls;
  ctx.assert(registration?.schema === 'multi-eg-telegram-registration-v1' && Array.isArray(calls)
    && calls.length === 3 && new Set(calls.map(c => c.request_id)).size === 3,
  'native_telegram_independent_registration_capture');
  for (const [i, method] of ['getMe', 'setWebhook', 'getWebhookInfo'].entries()) {
    const call = calls[i];
    ctx.assert(call.method === method && call.status === 200 && text(call.request_id)
      && call.bot_id === e.botId && hash(call.artifact_sha256) && call.response?.ok === true,
    'native_telegram_registration_actual_platform_call');
  }
  ctx.assert(same(calls[0].response.result, {id: e.botId, username: e.botUsername, is_bot: true})
    && same(calls[1].request, {url: e.callbackUrl, secret_sha256: e.webhookSecretSha256,
      allowed_updates: updates, drop_pending_updates: false}) && calls[1].response.result === true
    && calls[2].response.result?.url === e.callbackUrl
    && calls[2].response.result.pending_update_count === 0, 'native_telegram_verified_webhook_activation');
}

export function assertNativeTelegramWire(ctx, lease, capture, ingress, input) {
  ctx.assert(capture?.schema === 'multi-eg-telegram-wire-v1'
    && capture.request_id === ingress.receipt.request_id && capture.method === 'POST'
    && capture.path === '/v1/telegram/' + segment(lease.channelId)
    && hash(capture.artifact_sha256) && hash(capture.body_sha256) && hash(capture.secret_header_sha256),
  'native_telegram_independent_update_capture');
  ctx.assert((capture.secret_header_sha256 === lease.expected.webhookSecretSha256)
    === (input.variant !== 'invalid_webhook_secret'), 'native_telegram_actual_secret_variant');
  ctx.assert(same(capture.facts, {updateId: input.updateId, messageId: input.messageId,
    senderId: input.senderId, chatId: input.chatId, chatType: input.isGroup ? 'group' : 'private',
    isBotSender: false, mentioned: input.mentioned, textDigest: digest(input.text)}),
  'native_telegram_exact_raw_update_facts');
}

// The parent verifies the real captured event_ref with its own synthetic key.
// Never derive this reply address from fixture inputs or decode an unsigned blob.
export function assertNativeTelegramAssociation(ctx, lease, capture, event, input, turn) {
  ctx.assert(text(event.eventRef) && capture?.schema === 'multi-eg-native-event-v1'
    && capture.event_ref_sha256 === digest(event.eventRef) && capture.seal_verified === true
    && hash(capture.artifact_sha256) && turn.amRequests.some(r => r.request_id === capture.request_id)
    && ['eventId', 'conversationId', 'responseId', 'agentId', 'consumerId'].every(k => capture[k] === event[k]),
  'native_telegram_actual_provider_event_ref');
  ctx.assert(same(capture.reply, {type: 'telegram', bot_id: lease.expected.botId,
    chat_id: input.chatId, message_id: input.messageId, thread_id: null})
    && same(capture.source, {type: 'telegram', platform: 'telegram', bot_id: lease.expected.botId})
    && capture.actor_id === input.senderId && capture.text_sha256 === digest(input.text),
  'native_telegram_sealed_original_message_association');
}

export function assertNativeTelegramDelivery(ctx, lease, receipt, event) {
  delivered(ctx, event, receipt);
  const call = receipt.platformReceipt;
  ctx.assert(call?.schema === 'multi-eg-telegram-delivery-v1' && text(call.request_id)
    && hash(call.artifact_sha256) && call.method === 'sendMessage' && call.bot_id === lease.expected.botId
    && call.status === 200 && same(call.request, {chat_id: event.destination,
      text_sha256: event.expectedContentDigest}) && call.response?.ok === true
    && call.response.result?.chat_id === event.destination && positive(call.response.result.message_id),
  'native_telegram_actual_send_message_destination_answer');
  // Ordinary native replies retain original event identity but do not add a
  // Telegram quote. Do not invent replyToMessageId from the incoming update.
  ctx.assert(receipt.replyToMessageId == null, 'native_telegram_no_invented_platform_quote');
}

export function assertNativeTelegramActivity(ctx, lease, page, turns) {
  assertActivityPage(ctx, page, turns.map(t => t.event.eventId));
  for (const {event, input} of turns) {
    const row = page.items.find(r => r.event_id === event.eventId), c = row.consumers[0];
    ctx.assert(same(row.source, {type: 'telegram', platform: 'telegram', bot_id: lease.expected.botId})
      && row.actor.id === input.senderId && row.conversation.id === input.chatId
      && row.conversation.kind === (input.isGroup ? 'group' : 'private') && row.consumers.length === 1
      && c.consumer_id === event.consumerId && c.agent_id === event.agentId && c.response_id === event.responseId
      && text(c.conversation_id) && c.state === 'completed' && c.deliveries.length === 1
      && c.deliveries[0].step === 'answer' && c.deliveries[0].outcome === 'confirmed'
      && ['dispatched_at', 'accepted_at', 'produced_at', 'completed_at'].every(k => Number.isFinite(Date.parse(c[k])))
      && Number.isFinite(Date.parse(c.deliveries[0].confirmed_at)), 'native_telegram_retained_native_turn');
  }
}

export const nativeTelegramCase = define(39, 'eg', ['user-fixtures'], async ctx => {
  let lease, expected, initial;
  const turns = [], receipts = [], requestIds = new Set();
  let updateId = randomInt(1, 2 ** 40), messageId = randomInt(1, 2 ** 30);
  const bounded = (method, args = {}) => {
    ctx.assert(!ctx.signal?.aborted, 'native_telegram_cancelled');
    return invoke(lease, method, {...args, signal: ctx.signal,
      timeoutMs: Math.max(1, Math.min(15000, ctx.remainingMs?.() ?? 15000))});
  };
  const request = (isGroup, replica, changes = {}) => ({platform: 'telegram', trustMode: 'native',
    replica, updateId: String(updateId++), messageId: String(messageId++), senderId: expected.allowedSender,
    chatId: isGroup ? expected.groupChat : expected.privateChat, isGroup, mentioned: isGroup, isBotSender: false,
    text: (isGroup ? '@' + expected.botUsername + ' ' : '') + 'Native message ' + randomUUID(),
    completionText: 'Native answer ' + randomUUID(), ...changes});
  const captureIngress = async (result, input, outcome) => {
    assertRelayResponse(ctx, result.ingress, input.replica, outcome);
    const receipt = result.ingress.receipt;
    ctx.assert(!requestIds.has(receipt.request_id), 'native_telegram_unique_ingress');
    requestIds.add(receipt.request_id); receipts.push(receipt);
    const capture = await bounded('callbackCapture', {requestId: receipt.request_id});
    assertNativeTelegramWire(ctx, lease, capture, result.ingress, input);
    await ctx.observe('native_telegram_ingress', async () => ({schema: 'multi-eg-request-v1', receipt}));
    await ctx.observe('native_telegram_wire', async () => ({artifact_sha256: capture.artifact_sha256,
      body_sha256: capture.body_sha256, request_sha256: digest(receipt.request_id)}));
  };
  const activity = async replica => {
    const value = await egRequest(ctx, lease, {path: '/v1/channels/' + segment(lease.channelId) + '/activity',
      replica, returnResponse: true, signal: ctx.signal});
    ctx.assert(validEgReceipt(value.eg_receipt), 'native_telegram_actual_activity_receipt');
    assertNativeTelegramActivity(ctx, lease, value.body, turns);
  };
  await ctx.step('step_1', async () => {
    lease = await acquire(ctx, 'native_telegram'); expected = validateNativeTelegramFixture(lease);
    for (const replica of [ctx.am, peer(ctx.am)]) {
      const record = await egRequest(ctx, lease, {path: '/v1/channels/' + segment(lease.channelId), replica, signal: ctx.signal});
      assertNativeTelegramSource(ctx, lease, record);
      if (initial) ctx.assert(same(record, initial), 'native_telegram_both_eg_same_source');
      else initial = record;
      await activity(replica);
    }
    const registration = await bounded('platformConfiguration', {platform: 'telegram'});
    assertNativeTelegramRegistration(ctx, lease, registration);
    await ctx.observe('native_telegram_activation', async () => ({registration_sha256: digest(registration), trust_mode: 'native'}));
    assertRelayCounters(ctx, await bounded('counters'), 0, 0);
  });
  await ctx.step('step_2', async () => {
    for (const isGroup of [false, true]) {
      const input = request(isGroup, isGroup ? peer(ctx.am) : ctx.am);
      const event = await bounded('platformSend', input);
      ctx.assert(event.eventId === 'telegram:' + expected.botId + ':' + input.updateId
        && event.platformMessageId === input.messageId && event.consumerId === expected.consumerId
        && event.platform === 'telegram' && event.trustMode === 'native' && event.destination === input.chatId
        && event.expectedSender === expected.botId && event.expectedContentDigest === digest(input.completionText),
      'native_telegram_exact_admitted_update');
      await captureIngress(event, input, event);
      const turn = await bounded('waitForTurn', {eventId: event.eventId}); await retainTurn(ctx, event, turn);
      const original = await bounded('nativeEventCapture', {eventId: event.eventId});
      assertNativeTelegramAssociation(ctx, lease, original, event, input, turn);
      const context = await bounded('providerContext', {eventId: event.eventId});
      assertJourneyContext(ctx, event, context, input.text, []);
      const receipt = await bounded('waitForDelivery', {eventId: event.eventId});
      assertNativeTelegramDelivery(ctx, lease, receipt, event); turns.push({event, input, turn, receipt});
      await ctx.observe('native_telegram_delivery', async () => ({event_sha256: digest(event.eventId),
        response_sha256: digest(event.responseId), original_sha256: digest(original),
        provider_context_sha256: digest(context), delivery_sha256: digest(receipt), trust_mode: 'native'}));
      assertRelayCounters(ctx, await bounded('drain', {eventIds: turns.map(t => t.event.eventId)}), turns.length, turns.length);
    }
    for (const replica of [ctx.am, peer(ctx.am)]) {
      for (const {input, outcome} of [
        {input: request(false, replica, {variant: 'invalid_webhook_secret'}), outcome: {detail: 'telegram_webhook_secret'}},
        {input: request(true, replica, {senderId: expected.deniedSender}), outcome: {reason: 'sender_denied'}},
        {input: request(true, replica, {mentioned: false, text: 'Unaddressed native message ' + randomUUID()}),
          outcome: {reason: 'group_not_addressed'}},
      ]) {
        await captureIngress(await bounded('platformSend', input), input, outcome);
        assertRelayCounters(ctx, await bounded('drain', {eventIds: turns.map(t => t.event.eventId)}), 2, 2);
      }
    }
  });
  await ctx.step('expected_result', async () => {
    ctx.assert(turns.length === 2 && turns[0].event.conversationId !== turns[1].event.conversationId
      && turns[0].event.responseId !== turns[1].event.responseId, 'native_telegram_private_group_isolation');
    ctx.assert(receipts.length === 8 && new Set(receipts.map(r => r.replica)).size === 2
      && new Set(receipts.map(r => r.serving.instance_id)).size === 2, 'native_telegram_both_actual_eg_processes');
    for (const replica of [ctx.am, peer(ctx.am)]) {
      await activity(replica);
      const record = await egRequest(ctx, lease, {path: '/v1/channels/' + segment(lease.channelId), replica, signal: ctx.signal});
      ctx.assert(same(record, initial), 'native_telegram_saved_configuration_unchanged');
    }
    assertRelayCounters(ctx, await bounded('drain', {eventIds: turns.map(t => t.event.eventId)}), 2, 2);
  });
});
