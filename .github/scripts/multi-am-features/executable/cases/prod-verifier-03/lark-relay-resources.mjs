import {randomUUID} from 'node:crypto';
import {setTimeout as delay} from 'node:timers/promises';
import {Blocked} from '../../runner.mjs';
import {validEgReceipt} from '../../evidence-scope.mjs';
import {digest, same, invoke, egRequest, peer, segment, retainTurn, delivered} from './helpers.mjs';
import {assertRelayResponse} from './relay-verification.mjs';
import {assertJourneyCapture, assertJourneyContext} from './relay-journey.mjs';
import {assertActivityPage} from './activity-observer.mjs';
import {assertLarkNativeActivity} from './lark-native-journey.mjs';
import {readLarkSources} from './lark-source-inspection.mjs';
import {validateLarkRelayRuntime, larkRelayAttachment, assertLarkRelayContext, assertLarkRelayResourceCalls,
  assertLarkRelayMessage, assertLarkEffectCapture, larkRelayUpdate, assertLarkRelaySaved} from './lark-relay-evidence.mjs';

const hash = value => /^[a-f0-9]{64}$/.test(value ?? '');
const text = value => typeof value === 'string' && value.length > 0;
const bounded = ctx => ({signal: ctx.signal, timeoutMs: Math.max(1, Math.floor(Math.min(15000, ctx.remainingMs?.() ?? 15000)))});
// A complete 1x1 opaque black PNG. This is synthetic media, not a credential.
const image = () => Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=', 'base64');

export function validateLarkRelayFixture(lease, initial) {
  validateLarkRelayRuntime(lease);
  for (const method of ['prepareRelayResource', 'platformSend', 'callbackCapture', 'relayEventCapture', 'waitForTurn',
    'providerContext', 'waitForDelivery', 'relayReplyCapture', 'relayMessageCapture', 'captureRelayResources',
    'captureRelayMessageEffects', 'captureEffects', 'drain'])
    if (typeof lease[method] !== 'function') throw new Blocked('lark_relay_primitive_' + method.toLowerCase());
  for (const region of lease.regions) {
    const definition = initial?.records.get(region.relay.channelId)?.definition;
    if (!same(definition?.partition, {version: 1, key: 'conversation_and_sender'})
      || !same(definition.reply, {auto_final: true, ack_after_ms: 0, sinks: [{type: 'source'}]})
      || definition.routing?.version !== 1 || !same(definition.routing.default_targets, ['default'])
      || (definition.routing.consumers ?? []).length !== 0 || definition.provider?.target?.type === 'existing_agent'
      || definition.profile?.environment?.session_affinity === 'source_user')
      throw new Blocked('lark_relay_isolated_durable_answer_policy_required');
  }
}

export function larkRelayInput(region, replica) {
  const nonce = randomUUID().replaceAll('-', ''), messageId = randomUUID(), marker = 'EGRELAY_' + nonce;
  const input = {platform: region.region, trustMode: 'nyxid_relay', messageId, correlationId: randomUUID(),
    platformMessageId: 'om_' + nonce, senderId: region.relay.senderId, chatId: region.relay.chatId,
    routeId: region.relay.routeId, isGroup: false, mentioned: false, isBotSender: false, replica,
    text: 'Selected relay resource ' + marker, completionText: 'Relay resource accepted ' + marker,
    marker, resource: {key: 'img_' + nonce, bytes: image(), contentType: 'image/png'},
    normalizedContent: {type: 'unknown', attachments: []}};
  input.rawEvent = {schema: '2.0', header: {event_id: randomUUID(), event_type: 'im.message.receive_v1', app_id: region.relay.binding.app_id},
    event: {sender: {sender_type: 'user', sender_id: {open_id: input.senderId}}, message: {
      message_id: input.platformMessageId, chat_id: input.chatId, chat_type: 'p2p', create_time: String(Date.now()),
      message_type: 'post', content: JSON.stringify({title: '', content: [[{tag: 'text', text: input.text}, {tag: 'img', image_key: input.resource.key}]]})}}};
  return input;
}

export function assertLarkRelayOriginal(ctx, region, input, event, turn, wire, captured) {
  ctx.assert(same(wire.raw_platform_data, input.rawEvent) && wire.raw_event_sha256 === digest(input.rawEvent)
    && same(wire.normalized_content, input.normalizedContent), 'lark_relay_actual_raw_post_not_normalized_text');
  ctx.assert(captured?.schema === 'multi-eg-lark-relay-event-v1' && captured.seal_verified === true
    && hash(captured.artifact_sha256) && captured.event_ref_sha256 === digest(event.eventRef)
    && turn.amRequests.some(r => r.request_id === captured.request_id)
    && ['eventId', 'conversationId', 'responseId', 'agentId', 'consumerId'].every(k => captured[k] === event[k])
    && same(captured.source, {type: 'nyxid_relay', platform: region.region, route_id: region.relay.routeId})
    && captured.actor_id === input.senderId && captured.text_sha256 === digest(input.text)
    && same(captured.attachments, [larkRelayAttachment(region, input)])
    && same(captured.reply, {type: 'nyxid_relay', platform: region.region, route_id: region.relay.routeId,
      message_id: input.messageId, thread_reply: false, reply_token_sha256: wire.reply_token_sha256}),
  'lark_relay_sealed_exact_post_and_selected_locator');
}

export function assertLarkRelayDelivery(ctx, lease, region, event, content, capture, replyTokenHash) {
  const exchange = capture?.exchange, platform = capture?.platform;
  ctx.assert(capture?.schema === 'multi-eg-lark-relay-delivery-v1' && hash(capture.artifact_sha256)
    && exchange?.provenance === 'receiver' && text(exchange.request_id) && hash(exchange.artifact_sha256)
    && exchange.method === 'POST' && exchange.path === '/api/v1/channel-relay/reply' && exchange.status === 200
    && exchange.origin === lease.relayRuntime.origin && exchange.source === lease.source && exchange.image === lease.image
    && [replyTokenHash, region.relay.channelKeySha256].includes(exchange.credential_sha256)
    && same(exchange.request, {message_id: event.eventId, reply: {text_sha256: digest(content), metadata: null}})
    && text(exchange.response?.message_id) && exchange.response.message_id !== event.eventId
    && /^om_[A-Za-z0-9_-]+$/.test(exchange.response.platform_message_id ?? ''), 'lark_relay_exact_actual_reply_exchange');
  ctx.assert(platform?.provenance === 'receiver' && text(platform.request_id) && hash(platform.artifact_sha256)
    && platform.request_id !== exchange.request_id && platform.platform === region.region
    && platform.message_id === exchange.response.platform_message_id && platform.chat_id === region.relay.chatId
    && platform.bot_id === region.relay.botId && platform.app_id === region.relay.binding.app_id
    && platform.text_sha256 === digest(content) && platform.count === 1 && platform.status === 'delivered',
  'lark_relay_actual_platform_sender_destination_text');
}

function receipt(ctx, lease, response, replica, method) {
  const r = response.eg_receipt;
  ctx.assert(validEgReceipt(r) && r.replica === replica && r.method === method && r.status === response.status
    && r.request_id === response.requestId && r.serving.source === lease.source && r.serving.image === lease.image,
  'lark_relay_actual_public_request_receipt');
}

export function assertLarkRelayImage(ctx, body) {
  ctx.assert(body?.index === 0 && body.kind === 'image' && body.variant === 'embedded' && body.status === 'ready'
    && body.reason === undefined && body.retry_after_ms === undefined && body.text === undefined && body.download === undefined
    && body.images?.length === 1 && body.images[0].origin === 'normalized'
    && ['image/png', 'image/jpeg'].includes(body.images[0].mime_type) && body.images[0].width === 1 && body.images[0].height === 1
    && Number.isSafeInteger(body.images[0].bytes) && body.images[0].bytes > 0 && body.images[0].bytes <= 1048576,
  'lark_relay_exact_normalized_selected_image');
}

export async function readLarkRelayImage(ctx, lease, region, event, replica) {
  const deadline = performance.now() + bounded(ctx).timeoutMs;
  for (;;) {
    ctx.assert(!ctx.signal?.aborted && performance.now() < deadline, 'lark_relay_image_deadline');
    const response = await egRequest(ctx, lease, {replica, actor: 'relay_' + region.region + '_agent', returnResponse: true,
      path: '/v1/events/' + segment(event.eventRef) + '/attachments/0?view=image', signal: ctx.signal,
      timeoutMs: Math.max(1, Math.floor(Math.min(bounded(ctx).timeoutMs, deadline - performance.now())))});
    receipt(ctx, lease, response, replica, 'GET');
    if (response.body?.status !== 'pending') {assertLarkRelayImage(ctx, response.body); return response;}
    const body = response.body, wait = body.retry_after_ms;
    ctx.assert(body.index === 0 && body.kind === 'image' && body.variant === 'embedded'
      && body.text === undefined && body.images === undefined && body.download === undefined
      && Number.isSafeInteger(wait) && wait > 0 && performance.now() + wait < deadline, 'lark_relay_bounded_pending_image');
    await delay(wait, undefined, {signal: ctx.signal});
  }
}

function boundedCalls(ctx, lease) {
  const call = (method, input = {}) => {
    ctx.assert(!ctx.signal?.aborted, 'lark_relay_cancelled'); return invoke(lease, method, {...input, ...bounded(ctx)});
  };
  const request = async (replica, path, actor = 'owner', extra = {}) => {
    const response = await egRequest(ctx, lease, {replica, path, actor, ...bounded(ctx), ...extra, returnResponse: true});
    receipt(ctx, lease, response, replica, extra.method ?? 'GET'); return response;
  };
  return {call, request};
}

export async function runLarkRelayResources(ctx, lease, initial, nativeTurns) {
  validateLarkRelayFixture(lease, initial);
  ctx.assert(nativeTurns?.length === 6, 'lark_relay_native_journeys_required');
  const admitted = [], manual = [], actions = [], restored = new Map(initial.records);
  const channelIds = lease.regions.flatMap(r => [r.native.channelId, r.relay.channelId]);
  const {call, request} = boundedCalls(ctx, lease);
  const drain = async () => {
    const count = await call('drain', {channelIds, eventIds: [...nativeTurns, ...admitted].map(t => t.event.eventId),
      operationKeys: [...manual, ...actions].map(t => t.key), includeMedia: true});
    const turns = 6 + admitted.length, replies = turns + manual.length;
    ctx.assert(count?.turns === turns && count.effects === replies + actions.length && count.deliveries === replies
      && count.messageEffects === actions.length && count.pending === 0 && count.unknown === 0 && count.lateEffects === 0
      && count.mediaPending === 0 && count.mediaUnknown === 0, 'lark_relay_all_owned_work_reconciled');
  };
  const resources = () => call('captureRelayResources', {channelIds});
  const mutations = () => call('captureRelayMessageEffects', {channelIds});
  await drain();
  ctx.assert(same(await resources(), []) && same(await mutations(), []), 'lark_relay_empty_initial_resource_and_effect_inventory');
  for (const [index, region] of lease.regions.entries()) {
    const replica = index === 0 ? ctx.am : peer(ctx.am), input = larkRelayInput(region, replica);
    const original = initial.records.get(region.relay.channelId);
    await call('prepareRelayResource', {region: region.region, channelId: region.relay.channelId,
      platformMessageId: input.platformMessageId, key: input.resource.key, bytes: input.resource.bytes,
      contentType: input.resource.contentType, binding: region.relay.binding});
    const sent = await call('platformSend', {...input, region: region.region, channelId: region.relay.channelId});
    assertRelayResponse(ctx, sent.ingress, replica, sent);
    ctx.assert(sent.ingress.receipt.serving.source === lease.source && sent.ingress.receipt.serving.image === lease.image,
      'lark_relay_callback_exact_build');
    ctx.assert(sent.eventId === input.messageId && sent.consumerId === 'default'
      && sent.destination === region.relay.chatId && sent.expectedSender === region.relay.botId
      && sent.expectedContentDigest === digest(input.completionText), 'lark_relay_actual_admitted_post');
    await ctx.observe('lark_relay_ingress', async () => ({schema: 'multi-eg-request-v1', receipt: sent.ingress.receipt}));
    const wire = await call('callbackCapture', {region: region.region, requestId: sent.ingress.receipt.request_id});
    assertJourneyCapture(ctx, wire, sent, input, {channelId: region.relay.channelId,
      expected: {...region.relay, verificationMode: 'native_event'}});
    const turn = await call('waitForTurn', {eventId: sent.eventId}), captured = await call('relayEventCapture', {eventId: sent.eventId});
    const event = {...sent, eventRef: captured.eventRef};
    await retainTurn(ctx, event, turn); assertLarkRelayOriginal(ctx, region, input, event, turn, wire, captured);
    const provider = await call('providerContext', {eventId: event.eventId});
    assertJourneyContext(ctx, event, provider, input.text, [input.marker]);
    ctx.assert(!JSON.stringify(provider.messages).includes(input.resource.key), 'lark_relay_provider_has_no_resource_key');
    for (const target of [replica, peer(replica)]) {
      const context = await request(target, '/v1/events/' + segment(event.eventRef) + '/context', 'relay_' + region.region + '_agent');
      assertLarkRelayContext(ctx, region, input, event, context.body);
      await readLarkRelayImage(ctx, lease, region, event, target);
    }
    const delivery = await call('waitForDelivery', {eventId: event.eventId}); delivered(ctx, event, delivery);
    assertLarkRelayDelivery(ctx, lease, region, event, input.completionText, delivery.platformReceipt, wire.reply_token_sha256);
    admitted.push({region, input, event, turn, delivery}); await drain();
    assertLarkRelayResourceCalls(ctx, lease, admitted, await resources());

    const actor = 'relay_' + region.region + '_agent', replyText = 'Selected message effects ' + randomUUID(), key = randomUUID();
    const replied = await request(peer(replica), '/v1/events/' + segment(event.eventRef) + '/replies', actor,
      {method: 'POST', body: {text: replyText}, key});
    const replyCapture = await call('relayReplyCapture', {requestId: replied.requestId});
    assertLarkRelayDelivery(ctx, lease, region, event, replyText, replyCapture, region.relay.channelKeySha256);
    const messageCapture = await call('relayMessageCapture', {messageRef: replied.body.message_ref, requestId: replied.requestId});
    assertLarkRelayMessage(ctx, region, event, replied.body, replyCapture, messageCapture);
    ctx.assert(replyCapture.exchange.response.platform_message_id === replied.body.delivery.platform_message_id
      && messageCapture.request_id === replied.requestId, 'lark_relay_public_reply_and_platform_join');
    manual.push({key, event, capture: replyCapture}); await drain();
    const messageId = replied.body.delivery.platform_message_id, messageRef = replied.body.message_ref;
    const messagePath = '/v1/messages/' + segment(messageRef);
    for (const target of [replica, peer(replica)]) {
      const cap = (await request(target, messagePath + '/capabilities', actor)).body;
      ctx.assert(cap?.delete === true && cap.reaction === true && cap.scope === 'confirmed_native_bot_message'
        && cap.max_reactions === 1 && cap.media_edit === undefined && cap.expires_at === messageCapture.authority.expires_at,
      'lark_relay_selected_message_effect_capabilities');
    }
    for (const action of ['reaction_set', 'reaction_clear', 'delete']) {
      const suffix = action === 'delete' ? '/delete' : '/reaction', operationKey = randomUUID();
      const body = action === 'delete' ? {} : {reaction: action === 'reaction_set' ? [{type: 'emoji', emoji: 'THUMBSUP'}] : [], is_big: false};
      const applied = await request(replica, messagePath + suffix, actor, {method: 'POST', body, key: operationKey});
      ctx.assert(same(applied.body?.delivery, {outcome: 'confirmed', platform_message_id: messageId}), 'lark_relay_public_effect_confirmed');
      actions.push({key: operationKey, region, event, action, messageId, requestId: applied.requestId}); await drain();
      const captures = await mutations(), observed = captures.filter(c => c.operation_key_sha256 === digest(operationKey));
      ctx.assert(observed.length === 1 && observed[0].eg_request_id === applied.requestId, 'lark_relay_effect_actual_ingress_join');
      assertLarkEffectCapture(ctx, lease, region, messageId, action, observed[0]);
      if (action === 'reaction_clear') {
        const set = captures.find(c => c.action === 'reaction_set' && c.messageRead.response.data.items[0].message_id === messageId);
        ctx.assert(set?.reactionId === observed[0].reactionId, 'lark_relay_clear_only_confirmed_reaction');
      }
      // Deliberate same-intent replay after a confirmed result; never recovery
      // from an unknown mutation. It must not call the platform again.
      const replay = await request(peer(replica), messagePath + suffix, actor, {method: 'POST', body, key: operationKey});
      ctx.assert(same(replay.body, applied.body), 'lark_relay_confirmed_effect_replays_same_receipt');
      await drain(); ctx.assert(same(await mutations(), captures), 'lark_relay_confirmed_replay_no_second_platform_effect');
    }
    const beforeRefusalResources = await resources(), beforeRefusalEffects = await mutations();
    for (const target of [replica, peer(replica)]) {
      const foreign = 'native_lark_' + region.region + '_agent';
      for (const path of ['/v1/events/' + segment(event.eventRef) + '/attachments/0?view=image', messagePath + '/capabilities']) {
        const denied = await request(target, path, foreign, {allowed: [403]});
        ctx.assert(denied.body?.code === 'forbidden' && denied.body.images === undefined && denied.body.delete === undefined,
          'lark_relay_native_consumer_has_no_selected_relay_authority');
      }
      const arbitraryConnection = region.connections.find(c => c.user_service_id !== region.relay.binding.user_service_id);
      const extraScope = await request(target, messagePath + '/reaction', actor, {method: 'POST',
        body: {reaction: [{type: 'emoji', emoji: 'THUMBSUP'}], connection_ref: arbitraryConnection.user_service_id},
        key: randomUUID(), allowed: [400]});
      ctx.assert(extraScope.body?.code === 'invalid_request' && extraScope.body.delivery === undefined,
        'lark_relay_caller_cannot_supply_unselected_connection');
    }
    // Remove both independent consents with the actual optimistic version. No
    // credentials or routing are patched; parent cleanup already owns deletion.
    const changed = await request(replica, '/v1/channels/' + segment(region.relay.channelId), 'owner',
      {method: 'PUT', body: larkRelayUpdate(original, false)});
    assertLarkRelaySaved(ctx, original, changed.body, false, 1);
    for (const target of [replica, peer(replica)]) {
      const fresh = (await request(target, '/v1/channels/' + segment(region.relay.channelId))).body;
      ctx.assert(same(fresh, changed.body) && fresh.definition.sources[0].lark_resources == null
        && fresh.definition.sources[0].lark_message_effects == null, 'lark_relay_removed_consent_visible_on_both_egs');
      const file = await request(target, '/v1/events/' + segment(event.eventRef) + '/attachments/0?view=image', actor, {allowed: [403]});
      ctx.assert(file.body?.code === 'forbidden' && file.body.detail === 'attachment_source_revoked'
        && file.body.images === undefined, 'lark_relay_warm_resource_refused_after_revocation');
      const effect = await request(target, messagePath + '/reaction', actor,
        {method: 'POST', body: {reaction: [{type: 'emoji', emoji: 'THUMBSUP'}]}, key: randomUUID(), allowed: [403]});
      ctx.assert(effect.body?.code === 'forbidden' && effect.body.detail === 'message_effect_revoked'
        && effect.body.delivery === undefined, 'lark_relay_old_message_refused_after_revocation');
    }
    await drain();
    ctx.assert(same(await resources(), beforeRefusalResources) && same(await mutations(), beforeRefusalEffects),
      'lark_relay_refusals_no_resource_or_message_effect');
    const restoredResponse = await request(peer(replica), '/v1/channels/' + segment(region.relay.channelId), 'owner',
      {method: 'PUT', body: {...larkRelayUpdate(original, true), expected_version: changed.body.version}});
    assertLarkRelaySaved(ctx, original, restoredResponse.body, true, 2);
    restored.set(region.relay.channelId, restoredResponse.body);
    await ctx.observe('lark_relay_resource_turn', async () => ({region: region.region, event_sha256: digest(event.eventId),
      wire_sha256: wire.artifact_sha256, original_sha256: digest(captured), provider_sha256: digest(provider),
      delivery_sha256: digest(delivery), message_authority_sha256: digest(messageCapture), restored_sha256: digest(restoredResponse.body)}));
  }
  await drain();
  return {nativeTurns, admitted, manual, actions, restored};
}

// Re-read actual retained state for the frozen expected_result step. A fixture's
// generic resource/effect verdict cannot stand in for these executed journeys.
export async function verifyLarkRelayResult(ctx, lease, initial, result) {
  if (!result || !Array.isArray(result.nativeTurns) || result.nativeTurns.length !== 6
    || !Array.isArray(result.admitted) || result.admitted.length !== 2
    || !Array.isArray(result.manual) || result.manual.length !== 2
    || !Array.isArray(result.actions) || result.actions.length !== 6 || !(result.restored instanceof Map))
    throw new Blocked('lark_relay_completed_journeys_required');
  const {nativeTurns, admitted, manual, actions, restored} = result, {call, request} = boundedCalls(ctx, lease);
  const channelIds = lease.regions.flatMap(r => [r.native.channelId, r.relay.channelId]);
  const count = await call('drain', {channelIds, eventIds: [...nativeTurns, ...admitted].map(t => t.event.eventId),
    operationKeys: [...manual, ...actions].map(t => t.key), includeMedia: true});
  ctx.assert(count?.turns === 8 && count.effects === 16 && count.deliveries === 10 && count.messageEffects === 6
    && count.pending === 0 && count.unknown === 0 && count.lateEffects === 0 && count.mediaPending === 0 && count.mediaUnknown === 0,
  'lark_relay_all_owned_work_reconciled');
  const resourceCalls = await call('captureRelayResources', {channelIds}), effectCalls = await call('captureRelayMessageEffects', {channelIds});
  assertLarkRelayResourceCalls(ctx, lease, admitted, resourceCalls);
  ctx.assert(effectCalls.length === 6 && actions.every(a => effectCalls.filter(c => c.operation_key_sha256 === digest(a.key)).length === 1),
    'lark_relay_exact_six_message_effects');
  for (const a of actions) assertLarkEffectCapture(ctx, lease, a.region, a.messageId, a.action,
    effectCalls.find(c => c.operation_key_sha256 === digest(a.key)));
  const turns = [...nativeTurns, ...admitted];
  ctx.assert(new Set(turns.map(t => t.event.responseId)).size === 8
    && new Set(turns.map(t => t.event.agentId)).size === 4 && new Set(turns.map(t => t.event.conversationId)).size === 4,
  'lark_relay_native_and_relay_contexts_isolated');
  ctx.assert(new Set(turns.flatMap(t => t.turn.amRequests.map(r => r.am))).size === 2,
    'lark_relay_native_and_relay_actual_am_receipts');
  for (const region of lease.regions) for (const replica of [ctx.am, peer(ctx.am)]) {
    const native = (await request(replica, '/v1/channels/' + segment(region.native.channelId) + '/activity')).body;
    assertLarkNativeActivity(ctx, region, native, nativeTurns);
    const own = admitted.find(t => t.region.region === region.region);
    const activity = (await request(replica, '/v1/channels/' + segment(region.relay.channelId) + '/activity')).body;
    assertActivityPage(ctx, activity, [own.event.eventId]);
    const row = activity.items[0], consumer = row.consumers[0];
    ctx.assert(same(row.source, {type: 'nyxid_relay', platform: region.region, route_id: region.relay.routeId})
      && row.actor.id === region.relay.senderId && row.conversation.id === region.relay.chatId
      && row.consumers.length === 1 && consumer.state === 'completed' && consumer.consumer_id === 'default'
      && consumer.agent_id === own.event.agentId && consumer.response_id === own.event.responseId,
    'lark_relay_retained_completed_selected_turn');
  }
  await readLarkSources(ctx, lease, {records: restored, capabilities: initial.capabilities});
  const allEffects = await call('captureEffects');
  const expectedRequests = [...nativeTurns.map(t => t.delivery.platformReceipt.request_id),
    ...admitted.map(t => t.delivery.platformReceipt.platform.request_id), ...manual.map(t => t.capture.platform.request_id),
    ...effectCalls.map(c => c.request_id)];
  ctx.assert(expectedRequests.length === 16 && new Set(expectedRequests).size === 16 && allEffects?.length === 16
    && allEffects.every(e => expectedRequests.includes(e.platformRequestId))
    && new Set(allEffects.map(e => e.platformRequestId)).size === 16, 'lark_relay_all_platform_effects_accounted');
  await ctx.observe('lark_relay_finality', async () => ({turns: 8, deliveries: 10, message_effects: 6,
    resources_sha256: digest(resourceCalls), effects_sha256: digest(allEffects), restored_sources_sha256: digest([...restored])}));
  return {turns, restored, resourceCalls, effectCalls};
}
