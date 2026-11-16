import {randomUUID} from 'node:crypto';
import {Blocked} from '../../runner.mjs';
import {validEgReceipt} from '../../evidence-scope.mjs';
import {define, acquire, invoke, egRequest, peer, segment, same, digest, retainTurn, delivered} from './helpers.mjs';
import {assertRelayCounters} from './relay-verification.mjs';
import {assertJourneyContext} from './relay-journey.mjs';
import {assertActivityPage} from './activity-observer.mjs';

const text = value => typeof value === 'string' && value.length > 0;
const hash = value => /^[a-f0-9]{64}$/.test(value ?? '');
const metaId = value => typeof value === 'string' && /^[1-9][0-9]{0,31}$/.test(value);
const messageId = value => typeof value === 'string' && /^wamid\.[!-~]{1,506}$/.test(value);
const source = e => ({type: 'whatsapp', app_id: e.appId, waba_id: e.accountId, phone_number_id: e.phoneId});
export const whatsappEventId = (e, id) => 'whatsapp:' + digest([e.appId, e.accountId, e.phoneId, id]);

export function validateNativeWhatsappFixture(lease) {
  const e = lease.expected;
  if (!text(lease.recordId) || !text(lease.channelId) || !e || e.consumerId !== 'default'
    || !text(e.ownerSubject) || !text(e.callbackUrl)
    || !['appId', 'accountId', 'phoneId', 'foreignAccountId', 'foreignPhoneId', 'allowedSender', 'deniedSender'].every(k => metaId(e[k]))
    || e.accountId === e.foreignAccountId || e.phoneId === e.foreignPhoneId || e.allowedSender === e.deniedSender
    || !hash(e.verifyTokenSha256) || !hash(e.appSecretSha256))
    throw new Blocked('native_whatsapp_isolated_seed_required');
  for (const method of ['request', 'platformConfiguration', 'webhookChallenge', 'platformSend', 'callbackCapture',
    'nativeEventCapture', 'waitForTurn', 'providerContext', 'waitForDelivery', 'captureEffects', 'counters', 'drain'])
    if (typeof lease[method] !== 'function') throw new Blocked('native_whatsapp_primitive_' + method.toLowerCase());
  return e;
}

export function assertWhatsappSource(ctx, lease, record) {
  const e = lease.expected, d = record?.definition, s = d?.sources?.[0], r = d?.routing;
  ctx.assert(record?.record_id === lease.recordId && record.channel_id === lease.channelId
    && record.endpoints?.whatsapp_webhook_url === e.callbackUrl && d?.owner?.subject === e.ownerSubject
    && record.provider?.slug === 'cma_codex' && d.sources?.length === 1
    && Object.entries(source(e)).every(([k, v]) => s[k] === v), 'native_whatsapp_saved_source_identity');
  ctx.assert(same(s.group_ids ?? [], []) && s.message_events !== true && s.forms_enabled !== true
    && s.admission?.type === 'scoped'
    && same(s.admission.senders, {type: 'allowlist', ids: [e.allowedSender]})
    && same(s.admission.chats, {type: 'allowlist', ids: [e.allowedSender]})
    && s.admission.groups === 'deny' && [undefined, 'deny'].includes(s.admission.bot_messages),
  'native_whatsapp_private_scoped_admission');
  // Native WhatsApp is enrolled in durable default-consumer routing at creation.
  ctx.assert(r?.version === 1 && same(r.default_consumer, {name: 'default'})
    && same(r.default_targets, ['default']) && same(r.consumers ?? [], []) && same(r.rules ?? [], [])
    && r.default_reply === undefined && [undefined, 'auto'].includes(r.reply_label)
    && same(d.partition, {version: 1, key: 'conversation_and_sender'})
    && same(d.reply, {auto_final: true, ack_after_ms: 0, sinks: [{type: 'source'}]})
    && d.provider?.target?.type !== 'existing_agent', 'native_whatsapp_durable_default_reply');
}

export function assertWhatsappCustody(ctx, lease, capture) {
  const e = lease.expected, calls = capture?.calls;
  ctx.assert(capture?.schema === 'multi-eg-whatsapp-custody-v1' && Array.isArray(calls) && calls.length === 2
    && new Set(calls.map(c => c.request_id)).size === 2 && calls.every(c => text(c.request_id)
      && c.method === 'GET' && c.status === 200 && hash(c.artifact_sha256)), 'native_whatsapp_actual_custody_calls');
  const token = calls[0], phones = calls[1], data = token.response?.data;
  ctx.assert(token.path === '/v25.0/debug_token' && data?.is_valid === true && data.app_id === e.appId
    && Array.isArray(data.scopes) && ['whatsapp_business_management', 'whatsapp_business_messaging'].every(s => data.scopes.includes(s))
    && phones.path === '/v25.0/' + e.accountId + '/phone_numbers'
    && same(phones.query, {fields: 'id', limit: '100'}) && Array.isArray(phones.response?.data)
    && phones.response.data.some(p => p.id === e.phoneId), 'native_whatsapp_verified_app_account_phone_custody');
}

export function assertWhatsappIngress(ctx, ingress, replica, method) {
  ctx.assert(validEgReceipt(ingress?.receipt) && ingress.receipt.replica === replica
    && ingress.receipt.method === method && ingress.receipt.status === ingress.status
    && ingress.protocolVersion === '1', 'native_whatsapp_actual_ingress_receipt');
}

export function assertWhatsappProblem(ctx, ingress, detail) {
  ctx.assert(ingress.status === 401 && ingress.contentType?.startsWith('application/problem+json')
    && ingress.body?.status === 401 && ingress.body.code === 'not_authenticated' && ingress.body.detail === detail,
  'native_whatsapp_exact_authentication_failure');
}

export function assertWhatsappChallenge(ctx, lease, result, capture, input) {
  const ingress = result.ingress;
  assertWhatsappIngress(ctx, ingress, input.replica, 'GET');
  ctx.assert(capture?.schema === 'multi-eg-whatsapp-challenge-v1' && capture.request_id === ingress.receipt.request_id
    && capture.method === 'GET' && capture.path === '/v1/whatsapp/' + segment(lease.channelId)
    && hash(capture.artifact_sha256) && hash(capture.verify_token_sha256)
    && capture.mode === 'subscribe' && capture.challenge_sha256 === digest(input.challenge)
    && Array.isArray(capture.query_keys) && same([...capture.query_keys].sort(), ['hub.challenge', 'hub.mode', 'hub.verify_token'])
    && (capture.verify_token_sha256 === lease.expected.verifyTokenSha256) === (input.verification === 'valid'),
  'native_whatsapp_actual_challenge_query');
  if (input.verification === 'valid') ctx.assert(ingress.status === 200
    && ingress.contentType?.startsWith('text/plain') && ingress.body === input.challenge,
  'native_whatsapp_exact_challenge_echo');
  else assertWhatsappProblem(ctx, ingress, 'native_whatsapp_verify_token');
}

export function assertWhatsappWire(ctx, lease, capture, ingress, input) {
  ctx.assert(capture?.schema === 'multi-eg-whatsapp-wire-v1' && capture.request_id === ingress.receipt.request_id
    && capture.method === 'POST' && capture.path === '/v1/whatsapp/' + segment(lease.channelId)
    && ['artifact_sha256', 'body_sha256', 'signature_sha256', 'expected_signature_sha256'].every(k => hash(capture[k]))
    && capture.signing_key_sha256 === lease.expected.appSecretSha256
    && (capture.signature_sha256 === capture.expected_signature_sha256) === (input.variant !== 'invalid_signature'),
  'native_whatsapp_independent_signed_body');
  // The payload proves WABA and phone facts; app identity comes from verified
  // source custody and signature validation, never an app_id field in the body.
  ctx.assert(same(capture.facts, {accountId: input.accountId, phoneId: input.phoneId,
    messageId: input.messageId, senderId: input.senderId, timestamp: input.timestamp,
    kind: 'text', groupId: null, textDigest: digest(input.text)}), 'native_whatsapp_exact_raw_envelope');
}

export function assertWhatsappResult(ctx, ingress, outcome) {
  if (outcome === 'invalid_signature') return assertWhatsappProblem(ctx, ingress, 'native_whatsapp_signature');
  const expected = outcome === 'accepted' ? {accepted: 1, refused: 0}
    : outcome === 'denied_sender' ? {accepted: 0, refused: 1} : {accepted: 0, refused: 0};
  ctx.assert(['accepted', 'denied_sender', 'wrong_phone', 'wrong_account'].includes(outcome)
    && ingress.status === 200 && ingress.contentType?.startsWith('application/json') && same(ingress.body, expected),
  'native_whatsapp_exact_accepted_refused_ignored_counts');
}

export function assertWhatsappAssociation(ctx, lease, capture, event, input, turn) {
  const e = lease.expected;
  ctx.assert(text(event.eventRef) && capture?.schema === 'multi-eg-native-event-v1'
    && capture.event_ref_sha256 === digest(event.eventRef) && capture.seal_verified === true
    && hash(capture.artifact_sha256) && turn.amRequests.some(r => r.request_id === capture.request_id)
    && ['eventId', 'conversationId', 'responseId', 'agentId', 'consumerId'].every(k => capture[k] === event[k]),
  'native_whatsapp_actual_provider_event_reference');
  ctx.assert(same(capture.source, source(e)) && capture.actor_id === input.senderId && capture.actor_kind === 'unknown'
    && capture.text_sha256 === digest(input.text) && same(capture.reply, {type: 'whatsapp', app_id: e.appId,
      waba_id: e.accountId, phone_number_id: e.phoneId, recipient_id: input.senderId, message_id: input.messageId}),
  'native_whatsapp_verified_private_reply_identity');
}

export function assertWhatsappDelivery(ctx, lease, receipt, event, input) {
  delivered(ctx, event, receipt);
  const call = receipt.platformReceipt;
  ctx.assert(call?.schema === 'multi-eg-whatsapp-delivery-v1' && text(call.request_id) && hash(call.artifact_sha256)
    && call.method === 'POST' && call.path === '/v25.0/' + lease.expected.phoneId + '/messages' && call.status === 200
    && same(call.request, {messaging_product: 'whatsapp', recipient_type: 'individual', to: input.senderId,
      context: {message_id: input.messageId}, type: 'text', text: {preview_url: false, body_sha256: event.expectedContentDigest}})
    && call.response?.messaging_product === 'whatsapp' && call.response.contacts?.length === 1
    && call.response.contacts[0].wa_id === input.senderId && call.response.messages?.length === 1
    && messageId(call.response.messages[0].id), 'native_whatsapp_actual_phone_parent_answer');
}

export function assertWhatsappActivity(ctx, lease, page, event) {
  assertActivityPage(ctx, page, event ? [event.eventId] : []);
  if (!event) return;
  const row = page.items[0], c = row.consumers[0];
  ctx.assert(same(row.source, source(lease.expected)) && row.actor.id === lease.expected.allowedSender
    && row.actor.kind === 'unknown' && row.conversation.id === event.destination && row.conversation.kind === 'private'
    && row.consumers.length === 1 && c.consumer_id === event.consumerId && c.agent_id === event.agentId
    && c.response_id === event.responseId && text(c.conversation_id) && c.state === 'completed'
    && c.deliveries.length === 1 && c.deliveries[0].step === 'answer' && c.deliveries[0].outcome === 'confirmed'
    && ['dispatched_at', 'accepted_at', 'produced_at', 'completed_at'].every(k => Number.isFinite(Date.parse(c[k])))
    && Number.isFinite(Date.parse(c.deliveries[0].confirmed_at)), 'native_whatsapp_retained_private_turn');
}

export const nativeWhatsappCase = define(41, 'eg', ['user-fixtures'], async ctx => {
  let lease, expected, initial, event, receipt, admittedInput;
  const requests = new Set(), receipts = [];
  const bounded = (method, args = {}) => {
    ctx.assert(!ctx.signal?.aborted, 'native_whatsapp_cancelled');
    return invoke(lease, method, {...args, signal: ctx.signal,
      timeoutMs: Math.max(1, Math.min(15000, ctx.remainingMs?.() ?? 15000))});
  };
  const retainIngress = async ingress => {
    ctx.assert(!requests.has(ingress.receipt.request_id), 'native_whatsapp_unique_request');
    requests.add(ingress.receipt.request_id); receipts.push(ingress.receipt);
    await ctx.observe('native_whatsapp_ingress', async () => ({schema: 'multi-eg-request-v1', receipt: ingress.receipt}));
  };
  const input = (replica, changes = {}) => ({platform: 'whatsapp', trustMode: 'native', replica,
    accountId: expected.accountId, phoneId: expected.phoneId, senderId: expected.allowedSender,
    messageId: 'wamid.' + randomUUID(), timestamp: Math.floor(Date.now() / 1000),
    text: 'Native WhatsApp message ' + randomUUID(), completionText: 'Native WhatsApp answer ' + randomUUID(), ...changes});
  const submit = async (request, outcome) => {
    const result = await bounded('platformSend', request);
    assertWhatsappIngress(ctx, result.ingress, request.replica, 'POST');
    assertWhatsappResult(ctx, result.ingress, outcome);
    const capture = await bounded('callbackCapture', {requestId: result.ingress.receipt.request_id});
    assertWhatsappWire(ctx, lease, capture, result.ingress, request); await retainIngress(result.ingress);
    await ctx.observe('native_whatsapp_wire', async () => ({artifact_sha256: capture.artifact_sha256,
      body_sha256: capture.body_sha256, request_sha256: digest(capture.request_id)}));
    return result;
  };
  const activity = async replica => {
    const value = await egRequest(ctx, lease, {path: '/v1/channels/' + segment(lease.channelId) + '/activity',
      replica, returnResponse: true, signal: ctx.signal});
    ctx.assert(validEgReceipt(value.eg_receipt), 'native_whatsapp_actual_activity_receipt');
    assertWhatsappActivity(ctx, lease, value.body, event);
  };
  await ctx.step('step_1', async () => {
    lease = await acquire(ctx, 'native_whatsapp'); expected = validateNativeWhatsappFixture(lease);
    for (const replica of [ctx.am, peer(ctx.am)]) {
      const record = await egRequest(ctx, lease, {path: '/v1/channels/' + segment(lease.channelId), replica, signal: ctx.signal});
      assertWhatsappSource(ctx, lease, record);
      if (initial) ctx.assert(same(record, initial), 'native_whatsapp_both_eg_same_source'); else initial = record;
      await activity(replica);
      for (const verification of ['valid', 'invalid']) {
        const request = {platform: 'whatsapp', replica, verification, challenge: randomUUID()};
        const result = await bounded('webhookChallenge', request);
        const capture = await bounded('callbackCapture', {requestId: result.ingress.receipt.request_id});
        assertWhatsappChallenge(ctx, lease, result, capture, request); await retainIngress(result.ingress);
        await ctx.observe('native_whatsapp_challenge', async () => ({artifact_sha256: capture.artifact_sha256,
          challenge_sha256: digest(request.challenge), status: result.ingress.status}));
      }
    }
    const custody = await bounded('platformConfiguration', {platform: 'whatsapp'});
    assertWhatsappCustody(ctx, lease, custody);
    await ctx.observe('native_whatsapp_custody', async () => ({custody_sha256: digest(custody)}));
    assertRelayCounters(ctx, await bounded('counters'), 0, 0);
  });
  await ctx.step('step_2', async () => {
    admittedInput = input(ctx.am); event = await submit(admittedInput, 'accepted');
    ctx.assert(event.eventId === whatsappEventId(expected, admittedInput.messageId) && event.platformMessageId === admittedInput.messageId
      && event.consumerId === 'default' && event.platform === 'whatsapp' && event.trustMode === 'native'
      && event.destination === expected.allowedSender && event.expectedSender === expected.phoneId
      && event.expectedContentDigest === digest(admittedInput.completionText), 'native_whatsapp_exact_private_event');
    const turn = await bounded('waitForTurn', {eventId: event.eventId}); await retainTurn(ctx, event, turn);
    const original = await bounded('nativeEventCapture', {eventId: event.eventId});
    assertWhatsappAssociation(ctx, lease, original, event, admittedInput, turn);
    const context = await bounded('providerContext', {eventId: event.eventId});
    assertJourneyContext(ctx, event, context, admittedInput.text, []);
    receipt = await bounded('waitForDelivery', {eventId: event.eventId});
    assertWhatsappDelivery(ctx, lease, receipt, event, admittedInput);
    await ctx.observe('native_whatsapp_answer', async () => ({event_sha256: digest(event.eventId),
      response_sha256: digest(event.responseId), original_sha256: digest(original),
      provider_context_sha256: digest(context), delivery_sha256: digest(receipt), trust_mode: 'native'}));
    assertRelayCounters(ctx, await bounded('drain', {eventIds: [event.eventId]}), 1, 1);
    for (const replica of [ctx.am, peer(ctx.am)]) for (const [outcome, changes] of [
      ['invalid_signature', {variant: 'invalid_signature'}], ['wrong_phone', {phoneId: expected.foreignPhoneId}],
      ['wrong_account', {accountId: expected.foreignAccountId}], ['denied_sender', {senderId: expected.deniedSender}],
    ]) {
      await submit(input(replica, changes), outcome);
      assertRelayCounters(ctx, await bounded('drain', {eventIds: [event.eventId]}), 1, 1);
    }
  });
  await ctx.step('expected_result', async () => {
    const effects = await bounded('captureEffects');
    ctx.assert(Array.isArray(effects) && effects.length === 1 && effects[0].step === 'answer'
      && effects[0].eventId === event.eventId && effects[0].responseId === event.responseId
      && effects[0].platformRequestId === receipt.platformReceipt.request_id && effects[0].attempts === 1,
    'native_whatsapp_one_answer_no_unconfigured_ack_or_read_effect');
    ctx.assert(receipts.length === 13 && new Set(receipts.map(r => r.replica)).size === 2
      && new Set(receipts.map(r => r.serving.instance_id)).size === 2, 'native_whatsapp_both_actual_eg_processes');
    for (const replica of [ctx.am, peer(ctx.am)]) {
      await activity(replica);
      const record = await egRequest(ctx, lease, {path: '/v1/channels/' + segment(lease.channelId), replica, signal: ctx.signal});
      ctx.assert(same(record, initial), 'native_whatsapp_preserves_source_and_reply_policy');
    }
    assertRelayCounters(ctx, await bounded('drain', {eventIds: [event.eventId]}), 1, 1);
  });
});
