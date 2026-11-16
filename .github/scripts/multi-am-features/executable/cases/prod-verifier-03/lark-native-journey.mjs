import {randomUUID} from 'node:crypto';
import {Blocked} from '../../runner.mjs';
import {validEgReceipt} from '../../evidence-scope.mjs';
import {createNativeLarkWebhook} from './native-lark-webhook.mjs';
import {digest, same, invoke, egRequest, peer, segment, retainTurn, delivered} from './helpers.mjs';
import {assertActivityPage} from './activity-observer.mjs';
import {assertRelayCounters} from './relay-verification.mjs';
import {assertJourneyContext} from './relay-journey.mjs';
import {validateLarkInspectionFixture, readLarkSources} from './lark-source-inspection.mjs';

const text = value => typeof value === 'string' && value.length > 0;
const hash = value => /^[a-f0-9]{64}$/.test(value ?? '');
const id = (value, prefix) => typeof value === 'string' && value.startsWith(prefix) && /^[A-Za-z0-9_-]{1,128}$/.test(value);
const required = (value, code) => {if (!value) throw new Blocked(code);};
const official = region => region === 'lark' ? 'https://open.larksuite.com' : 'https://open.feishu.cn';
const nativeSource = region => ({type: 'native_lark', ...region.native.identity});
const bound = ctx => ({signal: ctx.signal, timeoutMs: Math.max(1, Math.min(15000, ctx.remainingMs?.() ?? 15000))});
const messageText = input => input.event.message.message_type === 'file' ? '' : JSON.parse(input.event.message.content).text;

export function nativeLarkEventId(identity, messageId) {
  // Rust serializes NativeLarkIdentity in this declaration order.
  const canonical = {region: identity.region, app_id: identity.app_id, tenant_key: identity.tenant_key, bot_open_id: identity.bot_open_id};
  return 'native-lark:' + digest(['message', canonical, messageId]);
}

export function validateLarkNativeFixture(lease) {
  validateLarkInspectionFixture(lease);
  for (const method of ['nativeCustody', 'nativeCallbackCapture', 'prepareNativeAnswer', 'waitForTurn', 'nativeEventCapture',
    'providerContext', 'waitForDelivery', 'captureEffects', 'drain'])
    required(typeof lease[method] === 'function', 'lark_native_primitive_' + method.toLowerCase());
  const runtime = lease.nativeRuntime, binding = runtime?.binding;
  required(binding?.schema === 'multi-eg-native-lark-local-binding-v1' && binding.source === lease.source
    && binding.image === lease.image && hash(binding.artifact_sha256) && Array.isArray(binding.regions)
    && binding.regions.length === 2 && same(binding.regions.map(r => r.region).sort(), ['feishu', 'lark']),
  'lark_native_supported_local_transport_required');
  required(runtime.transport?.expectedSource === lease.source && runtime.transport.expectedImage === lease.image,
    'lark_native_callback_transport_same_build_required');
  for (const region of lease.regions) {
    const selected = binding.regions.find(r => r.region === region.region);
    let local; try {local = new URL(selected.local_origin);} catch {}
    required(selected.official_origin === official(region.region) && local && ['http:', 'https:'].includes(local.protocol)
      && ['127.0.0.1', '[::1]'].includes(local.hostname) && !local.username && !local.password
      && local.pathname === '/' && !local.search && !local.hash, 'lark_native_regional_loopback_binding_required');
    const seed = region.native, credentials = runtime.credentials?.[region.region];
    required(seed.senderId !== seed.identity.bot_open_id && hash(seed.sourceRevision)
      && ['app_secret', 'encrypt_key', 'verification_token'].every(k => hash(seed.credentialDigests?.[k]))
      && typeof credentials?.encrypt_key === 'string' && typeof credentials?.verification_token === 'string'
      && digest(credentials.encrypt_key) === seed.credentialDigests.encrypt_key
      && digest(credentials.verification_token) === seed.credentialDigests.verification_token,
    'lark_native_synthetic_callback_custody_required');
  }
}

function platformCall(ctx, lease, region, call, method, path) {
  const binding = lease.nativeRuntime.binding.regions.find(r => r.region === region.region);
  ctx.assert(call && text(call.request_id) && hash(call.artifact_sha256) && call.method === method && call.path === path
    && call.status === 200 && call.origin === official(region.region) && call.transport_origin === binding.local_origin
    && call.source === lease.source && call.image === lease.image && same(call.identity, region.native.identity),
  'lark_native_actual_regional_platform_call');
}

export function assertLarkNativeCustody(ctx, lease, region, capture) {
  ctx.assert(capture?.schema === 'multi-eg-native-lark-custody-v1' && hash(capture.artifact_sha256)
    && capture.record_id === region.native.recordId && Array.isArray(capture.calls) && capture.calls.length === 2,
  'lark_native_independent_custody_capture');
  const [token, bot] = capture.calls;
  platformCall(ctx, lease, region, token, 'POST', '/open-apis/auth/v3/tenant_access_token/internal');
  platformCall(ctx, lease, region, bot, 'GET', '/open-apis/bot/v3/info');
  ctx.assert(token.request_id !== bot.request_id
    && same(token.request, {app_id: region.native.identity.app_id, app_secret_sha256: region.native.credentialDigests.app_secret})
    && token.response?.code === 0 && hash(token.response.tenant_access_token_sha256)
    && Number.isSafeInteger(token.response.expire) && token.response.expire > 0 && token.response.expire <= 7200
    && bot.credential_sha256 === token.response.tenant_access_token_sha256
    && bot.response?.code === 0 && bot.response.bot?.open_id === region.native.identity.bot_open_id,
  'lark_native_actual_app_token_and_bot_identity');
}

export function assertLarkNativeIngress(ctx, lease, region, input, response, capture, expected) {
  const receipt = response?.eg_receipt, sent = response?.sent;
  ctx.assert(validEgReceipt(receipt) && receipt.replica === input.replica && receipt.method === 'POST'
    && receipt.request_id === response.requestId && receipt.status === response.status
    && receipt.serving.source === lease.source && receipt.serving.image === lease.image,
  'lark_native_actual_callback_receipt');
  ctx.assert(sent?.schema === 'multi-eg-native-lark-sender-v1' && sent.provenance === 'sender'
    && sent.kind === (input.challenge ? 'challenge' : 'event') && sent.variant === (input.variant ?? 'valid')
    && capture?.schema === 'multi-eg-native-lark-wire-v1' && capture.provenance === 'receiver'
    && capture.request_id === response.requestId && capture.method === 'POST'
    && capture.path === '/v1/native-lark/' + segment(region.native.channelId)
    && hash(capture.artifact_sha256) && hash(capture.body_sha256) && capture.body_sha256 === sent.body_sha256
    && hash(capture.payload_sha256) && capture.payload_sha256 === sent.payload_sha256
    && capture.payload_sha256 === digest(capture.payload)
    && capture.signature_sha256 === sent.signature_sha256 && capture.nonce_sha256 === sent.nonce_sha256
    && capture.timestamp === sent.timestamp && capture.encrypt_key_sha256 === region.native.credentialDigests.encrypt_key
    && sent.encrypt_key_sha256 === capture.encrypt_key_sha256
    && capture.verification_token_sha256 === sent.verification_token_sha256,
  'lark_native_independent_encrypted_wire_capture');
  ctx.assert((capture.verification_token_sha256 === region.native.credentialDigests.verification_token)
    === (input.variant !== 'invalid_token'), 'lark_native_actual_verification_token_variant');
  if (input.challenge) {
    ctx.assert(same(capture.payload, {type: 'url_verification', challenge: input.challenge})
      && capture.signature_sha256 === null && capture.nonce_sha256 === null && capture.timestamp === null,
    'lark_native_encrypted_unsigned_challenge');
  } else {
    const payload = capture.payload;
    ctx.assert(payload.schema === '2.0' && payload.header?.event_id === input.eventId
      && payload.header.event_type === input.eventType && same(payload.event, input.event)
      && !Object.hasOwn(payload.header, 'token')
      && (payload.header.app_id === region.native.identity.app_id) === (input.variant !== 'wrong_app')
      && (payload.header.tenant_key === region.native.identity.tenant_key) === (input.variant !== 'wrong_tenant')
      && hash(capture.signature_sha256) && hash(capture.nonce_sha256), 'lark_native_exact_received_event');
  }
  ctx.assert(response.status === expected.status && (expected.status === 401
    ? response.body?.type === 'about:blank' && response.body.title === 'not authenticated'
      && response.body.status === 401 && response.body.code === 'not_authenticated'
      && response.body.detail === 'native_lark_authentication_failed' && response.body.accepted === undefined
    : same(response.body, expected.body)), 'lark_native_exact_admission_response');
}

export function assertLarkNativeAssociation(ctx, region, input, event, turn, capture) {
  const message = input.event.message;
  ctx.assert(event.eventId === nativeLarkEventId(region.native.identity, message.message_id) && event.consumerId === 'default'
    && text(event.eventRef) && capture?.schema === 'multi-eg-native-event-v1' && capture.seal_verified === true
    && hash(capture.artifact_sha256) && capture.event_ref_sha256 === digest(event.eventRef)
    && turn.amRequests.some(r => r.request_id === capture.request_id)
    && ['eventId', 'conversationId', 'responseId', 'agentId', 'consumerId'].every(k => capture[k] === event[k]),
  'lark_native_sealed_event_from_actual_am_request');
  ctx.assert(same(capture.source, nativeSource(region)) && capture.actor_id === region.native.senderId
    && capture.text_sha256 === digest(messageText(input))
    && same(capture.reply, {type: 'native_lark', mentioned: false, identity: region.native.identity,
      chat_id: message.chat_id, message_id: message.message_id, thread_id: null, revision: region.native.sourceRevision}),
  'lark_native_original_app_message_and_revision');
}

export function assertLarkNativeContext(ctx, region, input, event, body, attachments = []) {
  ctx.assert(body?.activity_verified === true && body.event_id === event.eventId
    && body.channel_id === region.native.channelId && body.conversation_id === event.conversationId
    && body.consumer?.id === 'default' && same(body.source, nativeSource(region))
    && body.activity?.actor?.id === region.native.senderId && body.activity.actor.kind === 'human'
    && same(body.activity.conversation, {id: region.native.chatId, kind: 'private'})
    && body.activity.kind?.type === 'message' && body.activity.kind.text === messageText(input)
    && same(body.activity.kind.attachments, attachments) && body.activity.kind.mentions_bot === false,
  'lark_native_public_context_exact_retained_event');
  ctx.assert(body.capabilities?.forms !== true && body.capabilities.buttons === 'numbered_text'
    && body.capabilities.cards === 'text' && body.capabilities.max_text_chars === 6000,
  'lark_native_disabled_controls_and_forms_not_advertised');
}

export function assertLarkNativeDelivery(ctx, lease, region, input, event, receipt) {
  delivered(ctx, event, receipt);
  const call = receipt.platformReceipt;
  ctx.assert(call?.schema === 'multi-eg-native-lark-delivery-v1', 'lark_native_platform_delivery_capture');
  assertLarkNativeCustody(ctx, lease, region, call.custody);
  platformCall(ctx, lease, region, call, 'POST', '/open-apis/im/v1/messages?receive_id_type=chat_id');
  ctx.assert(call.credential_sha256 === call.custody.calls[0].response.tenant_access_token_sha256
    && !call.custody.calls.some(c => c.request_id === call.request_id)
    && same(call.request, {receive_id: region.native.chatId, msg_type: 'text',
    content_sha256: digest(JSON.stringify({text: input.completionText}))})
    && call.response?.code === 0 && id(call.response.data?.message_id, 'om_')
    && call.response.data.message_id !== input.event.message.message_id
    && call.response.data.chat_id === region.native.chatId && [undefined, ''].includes(call.response.data.thread_id)
    && receipt.replyToMessageId == null, 'lark_native_exact_chat_answer_no_invented_quote');
}

export function assertLarkNativeActivity(ctx, region, page, turns) {
  const own = turns.filter(t => t.region === region.region);
  assertActivityPage(ctx, page, own.map(t => t.event.eventId));
  for (const {event, input} of own) {
    const row = page.items.find(r => r.event_id === event.eventId), consumer = row.consumers[0];
    ctx.assert(same(row.source, nativeSource(region)) && row.actor.id === region.native.senderId && row.actor.kind === 'human'
      && same(row.conversation, {id: region.native.chatId, kind: 'private'}) && row.consumers.length === 1
      && consumer.consumer_id === 'default' && consumer.agent_id === event.agentId && consumer.response_id === event.responseId
      && consumer.state === 'completed' && text(consumer.conversation_id) && consumer.deliveries.length === 1
      && consumer.deliveries[0].step === 'answer' && consumer.deliveries[0].outcome === 'confirmed'
      && ['dispatched_at', 'accepted_at', 'produced_at', 'completed_at'].every(k => Number.isFinite(Date.parse(consumer[k])))
      && Number.isFinite(Date.parse(consumer.deliveries[0].confirmed_at)) && input.event.message.chat_type === 'p2p',
    'lark_native_retained_exact_completed_turn');
  }
}

// This runs native intake only. Relay attachments and connection-scoped effects
// remain a separate required journey, never implied by this function's return.
export async function runLarkNativeIntake(ctx, lease, initial) {
  validateLarkNativeFixture(lease);
  const senders = new Map(), turns = [], requestIds = new Set(), receipts = [], probeEventIds = [];
  const call = (method, input = {}) => {
    ctx.assert(!ctx.signal?.aborted, 'lark_native_cancelled');
    return invoke(lease, method, {...input, ...bound(ctx)});
  };
  const drain = () => call('drain', {eventIds: turns.map(t => t.event.eventId), probeEventIds: [...probeEventIds],
    channelIds: lease.regions.flatMap(r => [r.native.channelId, r.relay.channelId])});
  // Bind all senders and register cancellation before any callback is submitted.
  // Deferred sender closes precede the already-registered parent lease cleanup.
  for (const region of lease.regions) {
    const sender = await createNativeLarkWebhook(lease.nativeRuntime.transport, {local: true, synthetic: true,
      channelId: region.native.channelId, identity: region.native.identity,
      credentials: lease.nativeRuntime.credentials[region.region]}, value => ctx.observe('lark_native_sender', async () => value));
    ctx.defer('close_native_lark_sender_' + region.region, () => sender.close()); senders.set(region.region, sender);
    const definition = initial?.records.get(region.native.channelId)?.definition;
    ctx.assert(same(definition?.partition, {version: 1, key: 'conversation_and_sender'})
      && same(definition.reply, {auto_final: true, ack_after_ms: 0, sinks: [{type: 'source'}]})
      && definition.routing?.version === 1 && same(definition.routing.default_targets, ['default'])
      && (definition.routing.consumers ?? []).length === 0 && definition.provider?.target?.type !== 'existing_agent'
      && definition.profile?.environment?.session_affinity !== 'source_user', 'lark_native_isolated_durable_reply_policy');
    const custody = await call('nativeCustody', {region: region.region, recordId: region.native.recordId});
    assertLarkNativeCustody(ctx, lease, region, custody);
    await ctx.observe('lark_native_custody', async () => ({region: region.region,
      capture_sha256: digest(custody), binding_sha256: lease.nativeRuntime.binding.artifact_sha256}));
  }
  const read = async (replica, path, actor = 'owner', allowed = [200]) => {
    const r = await egRequest(ctx, lease, {replica, path, actor, allowed, ...bound(ctx), returnResponse: true});
    ctx.assert(validEgReceipt(r.eg_receipt) && r.eg_receipt.replica === replica && r.eg_receipt.status === r.status
      && r.eg_receipt.serving.source === lease.source && r.eg_receipt.serving.image === lease.image,
    'lark_native_actual_public_read_receipt'); return r;
  };
  const activity = async () => {
    for (const replica of [ctx.am, peer(ctx.am)]) for (const region of lease.regions) {
      const native = await read(replica, '/v1/channels/' + segment(region.native.channelId) + '/activity');
      assertLarkNativeActivity(ctx, region, native.body, turns);
      const relay = await read(replica, '/v1/channels/' + segment(region.relay.channelId) + '/activity');
      assertActivityPage(ctx, relay.body, []);
    }
  };
  const send = async (region, input, expected) => {
    const sender = senders.get(region.region);
    const response = await (input.challenge ? sender.sendChallenge({...input, allowed: [expected.status], ...bound(ctx)})
      : sender.sendEvent({...input, allowed: [expected.status], ...bound(ctx)}));
    const capture = await call('nativeCallbackCapture', {region: region.region, requestId: response.requestId});
    assertLarkNativeIngress(ctx, lease, region, input, response, capture, expected);
    if (!input.challenge && response.body?.accepted !== true)
      probeEventIds.push(nativeLarkEventId(region.native.identity, input.event.message.message_id));
    ctx.assert(!requestIds.has(response.requestId), 'lark_native_unique_callback_request'); requestIds.add(response.requestId);
    receipts.push(response.eg_receipt);
    await ctx.observe('lark_native_callback', async () => ({schema: 'multi-eg-request-v1', receipt: response.eg_receipt}));
    await ctx.observe('lark_native_wire', async () => ({region: region.region, artifact_sha256: capture.artifact_sha256,
      body_sha256: capture.body_sha256, payload_sha256: capture.payload_sha256}));
  };
  const input = (region, replica) => ({replica, eventId: randomUUID(), eventType: 'im.message.receive_v1',
    completionText: 'Native Lark answer ' + randomUUID(), event: {
      sender: {sender_type: 'user', sender_id: {open_id: region.native.senderId}, tenant_key: region.native.identity.tenant_key},
      message: {message_id: 'om_' + randomUUID().replaceAll('-', ''), chat_id: region.native.chatId, chat_type: 'p2p',
        create_time: String(Date.now()), message_type: 'text', content: JSON.stringify({text: 'Native Lark input ' + randomUUID()})}}});
  await activity(); assertRelayCounters(ctx, await call('counters'), 0, 0);
  for (const region of lease.regions) {
    for (const replica of [ctx.am, peer(ctx.am)]) {
      const challenge = randomUUID(); await send(region, {replica, challenge}, {status: 200, body: {challenge}});
      const current = input(region, replica), eventId = nativeLarkEventId(region.native.identity, current.event.message.message_id);
      await call('prepareNativeAnswer', {region: region.region, channelId: region.native.channelId, eventId,
        completionText: current.completionText});
      await send(region, current, {status: 200, body: {accepted: true}});
      const turn = await call('waitForTurn', {eventId}), captured = await call('nativeEventCapture', {eventId});
      const event = {...turn, eventId, eventRef: captured.eventRef, destination: region.native.chatId,
        expectedSender: region.native.identity.bot_open_id, expectedContentDigest: digest(current.completionText)};
      await retainTurn(ctx, event, turn); assertLarkNativeAssociation(ctx, region, current, event, turn, captured);
      const provider = await call('providerContext', {eventId});
      assertJourneyContext(ctx, event, provider, JSON.parse(current.event.message.content).text, []);
      const actor = 'native_lark_' + region.region + '_agent', path = '/v1/events/' + segment(event.eventRef) + '/context';
      const context = await read(peer(replica), path, actor); assertLarkNativeContext(ctx, region, current, event, context.body);
      const foreign = await read(replica, path, 'relay_' + region.region + '_agent', [403]);
      ctx.assert(foreign.body?.code === 'forbidden' && foreign.body.activity === undefined && foreign.body.capabilities === undefined,
        'lark_native_reference_not_relay_consumer_authority');
      const delivery = await call('waitForDelivery', {eventId}); assertLarkNativeDelivery(ctx, lease, region, current, event, delivery);
      turns.push({region: region.region, input: current, event, turn, delivery});
      await ctx.observe('lark_native_completed_turn', async () => ({region: region.region, event_sha256: digest(eventId),
        original_sha256: digest(captured), provider_sha256: digest(provider), context_sha256: digest(context.body), delivery_sha256: digest(delivery)}));
      assertRelayCounters(ctx, await drain(), turns.length, turns.length);
    }
    for (const replica of [ctx.am, peer(ctx.am)]) {
      for (const variant of ['invalid_signature', 'wrong_app', 'wrong_tenant']) {
        await send(region, {...input(region, replica), variant}, {status: 401,
          body: {code: 'not_authenticated', status: 401, detail: 'native_lark_authentication_failed'}});
      }
      const bot = input(region, replica); bot.event.sender.sender_type = 'app';
      await send(region, bot, {status: 200, body: {accepted: false, reason: 'unsupported_event'}});
      const denied = input(region, replica); denied.event.sender.sender_id.open_id = 'ou_' + randomUUID().replaceAll('-', '');
      await send(region, denied, {status: 200, body: {accepted: false, reason: 'native_lark_admission_refused'}});
      assertRelayCounters(ctx, await drain(), turns.length, turns.length);
    }
  }
  ctx.assert(turns.length === 4 && new Set(turns.map(t => t.event.eventId)).size === 4
    && new Set(turns.map(t => t.event.responseId)).size === 4, 'lark_native_four_actual_turns');
  for (const region of lease.regions) {
    const own = turns.filter(t => t.region === region.region);
    ctx.assert(own[0].event.conversationId === own[1].event.conversationId && own[0].event.agentId === own[1].event.agentId,
      'lark_native_original_chat_continuity');
  }
  ctx.assert(new Set(turns.map(t => t.event.conversationId)).size === 2 && new Set(turns.map(t => t.event.agentId)).size === 2,
    'lark_native_regional_conversation_isolation');
  ctx.assert(new Set(turns.map(t => t.turn.servingAm)).size === 2, 'lark_native_both_actual_non_owner_am_ingresses');
  ctx.assert(receipts.length === 28 && new Set(receipts.map(r => r.replica)).size === 2
    && new Set(receipts.map(r => r.serving.instance_id)).size === 2, 'lark_native_both_actual_eg_processes');
  const effects = await call('captureEffects');
  ctx.assert(Array.isArray(effects) && effects.length === 4 && new Set(effects.map(e => e.platformRequestId)).size === 4
    && turns.every(t => effects.filter(e => e.eventId === t.event.eventId && e.responseId === t.event.responseId
      && e.platformRequestId === t.delivery.platformReceipt.request_id && e.kind === 'native_text'
      && e.artifact_sha256 === t.delivery.platformReceipt.artifact_sha256).length === 1), 'lark_native_no_extra_or_relay_effects');
  await activity(); await readLarkSources(ctx, lease, initial);
  assertRelayCounters(ctx, await drain(), 4, 4);
  await ctx.observe('lark_native_finality', async () => ({turns: 4, deliveries: 4, effects_sha256: digest(effects),
    relay_resource_access_verified: false}));
  return turns;
}
