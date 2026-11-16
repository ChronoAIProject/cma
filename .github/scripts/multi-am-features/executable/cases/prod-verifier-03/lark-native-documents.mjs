import {randomUUID} from 'node:crypto';
import {setTimeout as delay} from 'node:timers/promises';
import {Blocked} from '../../runner.mjs';
import {validEgReceipt} from '../../evidence-scope.mjs';
import {digest, same, invoke, egRequest, peer, segment, retainTurn} from './helpers.mjs';
import {createNativeLarkWebhook} from './native-lark-webhook.mjs';
import {validateLarkNativeFixture, nativeLarkEventId, assertLarkNativeIngress, assertLarkNativeAssociation,
  assertLarkNativeContext, assertLarkNativeCustody, assertLarkNativeDelivery, assertLarkNativeActivity} from './lark-native-journey.mjs';
import {assertJourneyContext} from './relay-journey.mjs';
import {assertRelayCounters} from './relay-verification.mjs';
import {assertActivityPage} from './activity-observer.mjs';
import {readLarkSources} from './lark-source-inspection.mjs';

const hash = value => /^[a-f0-9]{64}$/.test(value ?? '');
const text = value => typeof value === 'string' && value.length > 0;
const bounds = ctx => ({signal: ctx.signal, timeoutMs: Math.max(1, Math.floor(Math.min(15000, ctx.remainingMs?.() ?? 15000)))});
const allChannels = lease => lease.regions.flatMap(r => [r.native.channelId, r.relay.channelId]);

export function validateLarkDocumentFixture(lease) {
  validateLarkNativeFixture(lease);
  // NativeLarkFileSource uses a different pinned transport from Client. A
  // client-only token/send injection cannot make its resource fetches local.
  const binding = lease.nativeMediaBinding;
  if (binding?.schema !== 'multi-eg-native-lark-media-binding-v1' || binding.source !== lease.source
    || binding.image !== lease.image || !hash(binding.artifact_sha256)
    || !same(binding.regions, lease.nativeRuntime.binding.regions))
    throw new Blocked('lark_native_supported_media_transport_required');
  for (const method of ['prepareNativeDocument', 'captureNativeResources'])
    if (typeof lease[method] !== 'function') throw new Blocked('lark_native_document_primitive_' + method.toLowerCase());
}

export function nativeLarkDocument(region, replica) {
  const nonce = randomUUID().replaceAll('-', ''), filename = 'native-' + nonce + '.txt';
  return {replica, eventId: randomUUID(), eventType: 'im.message.receive_v1',
    completionText: 'Native document answer ' + randomUUID(),
    document: {fileKey: 'file_' + nonce, filename, content: 'Native document ' + randomUUID() + '\nLark / Feishu 文档\n'},
    event: {sender: {sender_type: 'user', sender_id: {open_id: region.native.senderId}, tenant_key: region.native.identity.tenant_key},
      message: {message_id: 'om_' + nonce, chat_id: region.native.chatId, chat_type: 'p2p',
        create_time: String(Date.now()), message_type: 'file', content: JSON.stringify({file_key: 'file_' + nonce, file_name: filename})}}};
}

export function larkDocumentAttachment(region, input) {
  const message = input.event.message, content = JSON.parse(message.content);
  return {reference: 'native-lark:' + message.message_id + ':' + content.file_key, kind: 'file', filename: content.file_name,
    source: {type: 'native_lark', message_id: message.message_id, resource_key: content.file_key,
      resource_kind: 'file', source_revision: region.native.sourceRevision}};
}

function actualRead(ctx, lease, response, replica) {
  const receipt = response.eg_receipt;
  ctx.assert(validEgReceipt(receipt) && receipt.replica === replica && receipt.status === response.status
    && receipt.method === 'GET' && receipt.request_id === response.requestId
    && receipt.serving.source === lease.source && receipt.serving.image === lease.image,
  'lark_document_actual_read_receipt');
}

export function assertLarkDocumentView(ctx, input, body, page = 1) {
  ctx.assert(body?.index === 0 && body.kind === 'file' && body.filename === input.document.filename
    && body.detected_type === 'text/plain' && body.download === undefined && body.images === undefined
    && body.retry_after_ms === undefined && body.notice === undefined,
  'lark_document_exact_file_view');
  ctx.assert(same(body.text, {source: 'document', content: page === 1 ? input.document.content : '',
    page, pages: 1, truncated: false}) && body.status === (page === 1 ? 'ready' : 'unavailable')
    && body.reason === (page === 1 ? undefined : 'not_found'), 'lark_document_exact_bytes_and_page');
}

// Poll only a pending idempotent GET. An unavailable view, unknown transport,
// changed process or elapsed budget fails without resubmitting an event.
export async function readLarkDocument(ctx, lease, region, input, event, replica, page = 1) {
  const deadline = performance.now() + bounds(ctx).timeoutMs;
  const path = '/v1/events/' + segment(event.eventRef) + '/attachments/0?view=text&page=' + page;
  for (;;) {
    ctx.assert(!ctx.signal?.aborted && performance.now() < deadline, 'lark_document_read_deadline');
    const response = await egRequest(ctx, lease, {replica, path, actor: 'native_lark_' + region.region + '_agent',
      returnResponse: true, signal: ctx.signal, timeoutMs: Math.max(1, Math.floor(Math.min(bounds(ctx).timeoutMs, deadline - performance.now())))});
    actualRead(ctx, lease, response, replica);
    if (response.body?.status !== 'pending') {
      assertLarkDocumentView(ctx, input, response.body, page); return response;
    }
    const body = response.body, wait = body.retry_after_ms;
    ctx.assert(body.index === 0 && body.kind === 'file' && body.filename === input.document.filename
      && Number.isSafeInteger(wait) && wait > 0 && wait <= 15000 && body.text === undefined
      && body.download === undefined && body.images === undefined, 'lark_document_bounded_pending_view');
    ctx.assert(performance.now() + wait < deadline, 'lark_document_read_deadline');
    await delay(wait, undefined, {signal: ctx.signal});
  }
}

export function assertLarkResourceCalls(ctx, lease, documents, calls) {
  ctx.assert(Array.isArray(calls) && calls.length >= documents.length
    && new Set(calls.map(c => c.request_id)).size === calls.length, 'lark_document_actual_resource_calls');
  const seen = new Set();
  for (const call of calls) {
    const document = documents.find(d => call.path === '/open-apis/im/v1/messages/' + d.input.event.message.message_id
      + '/resources/' + d.input.document.fileKey + '?type=file');
    ctx.assert(document, 'lark_document_no_unscoped_resource_fetch');
    const region = lease.regions.find(r => r.region === document.region);
    const binding = lease.nativeMediaBinding.regions.find(r => r.region === document.region);
    ctx.assert(call.schema === 'multi-eg-native-lark-resource-v1' && text(call.request_id) && hash(call.artifact_sha256)
      && call.method === 'GET' && call.status === 200 && call.origin === binding.official_origin
      && call.transport_origin === binding.local_origin && call.source === lease.source && call.image === lease.image
      && same(call.identity, region.native.identity) && call.content_type === 'text/plain'
      && call.body_sha256 === digest(document.input.document.content)
      && call.bytes === Buffer.byteLength(document.input.document.content)
      && call.connection_ref === undefined && call.user_service_id === undefined,
    'lark_document_native_exact_resource_bytes');
    assertLarkNativeCustody(ctx, lease, region, call.custody);
    ctx.assert(call.credential_sha256 === call.custody.calls[0].response.tenant_access_token_sha256
      && !call.custody.calls.some(c => c.request_id === call.request_id), 'lark_document_resource_token_custody');
    seen.add(document.event.eventId);
  }
  ctx.assert(seen.size === documents.length, 'lark_document_each_original_file_fetched');
}

// Native documents use their app custody. Selected NyxID relay resources and
// message effects are a separate required journey and remain unverified here.
export async function runLarkNativeDocuments(ctx, lease, initial, earlierTurns) {
  validateLarkDocumentFixture(lease);
  ctx.assert(earlierTurns?.length === 4, 'lark_document_prior_native_intake_required');
  const turns = [...earlierTurns], documents = [], requestIds = new Set();
  const call = (method, input = {}) => {
    ctx.assert(!ctx.signal?.aborted, 'lark_document_cancelled');
    return invoke(lease, method, {...input, ...bounds(ctx)});
  };
  const drain = async () => {
    const counters = await call('drain', {eventIds: turns.map(t => t.event.eventId), channelIds: allChannels(lease), includeMedia: true});
    assertRelayCounters(ctx, counters, turns.length, turns.length);
    ctx.assert(counters.mediaPending === 0 && counters.mediaUnknown === 0, 'lark_document_media_work_joined');
  };
  const resources = () => call('captureNativeResources', {channelIds: allChannels(lease)});
  const read = async (replica, path, actor = 'owner', allowed = [200]) => {
    const response = await egRequest(ctx, lease, {replica, path, actor, allowed, returnResponse: true, ...bounds(ctx)});
    actualRead(ctx, lease, response, replica); return response;
  };
  await drain(); ctx.assert(same(await resources(), []), 'lark_document_no_prior_resource_fetches');
  for (const [index, region] of lease.regions.entries()) {
    const replica = index === 0 ? ctx.am : peer(ctx.am), input = nativeLarkDocument(region, replica);
    const eventId = nativeLarkEventId(region.native.identity, input.event.message.message_id);
    const sender = await createNativeLarkWebhook(lease.nativeRuntime.transport, {local: true, synthetic: true,
      channelId: region.native.channelId, identity: region.native.identity, credentials: lease.nativeRuntime.credentials[region.region]});
    ctx.defer('close_lark_document_sender_' + region.region, () => sender.close());
    // This only stages bytes on the existing parent's synthetic platform.
    await call('prepareNativeDocument', {region: region.region, channelId: region.native.channelId, eventId,
      messageId: input.event.message.message_id, fileKey: input.document.fileKey,
      filename: input.document.filename, content: input.document.content, contentType: 'text/plain'});
    await call('prepareNativeAnswer', {region: region.region, channelId: region.native.channelId, eventId,
      completionText: input.completionText});
    const response = await sender.sendEvent({...input, ...bounds(ctx)});
    const wire = await call('nativeCallbackCapture', {region: region.region, requestId: response.requestId});
    assertLarkNativeIngress(ctx, lease, region, input, response, wire, {status: 200, body: {accepted: true}});
    ctx.assert(!requestIds.has(response.requestId), 'lark_document_distinct_callback_request'); requestIds.add(response.requestId);
    await ctx.observe('lark_document_ingress', async () => ({schema: 'multi-eg-request-v1', receipt: response.eg_receipt}));
    const turn = await call('waitForTurn', {eventId}), captured = await call('nativeEventCapture', {eventId});
    const event = {...turn, eventId, eventRef: captured.eventRef, destination: region.native.chatId,
      expectedSender: region.native.identity.bot_open_id, expectedContentDigest: digest(input.completionText)};
    await retainTurn(ctx, event, turn); assertLarkNativeAssociation(ctx, region, input, event, turn, captured);
    const attachment = larkDocumentAttachment(region, input);
    ctx.assert(same(captured.attachments, [attachment]), 'lark_document_sealed_original_locator');
    const provider = await call('providerContext', {eventId});
    assertJourneyContext(ctx, event, provider, input.document.filename, []);
    ctx.assert(!JSON.stringify(provider.messages).includes(input.document.fileKey), 'lark_document_no_provider_resource_locator');
    for (const target of [replica, peer(replica)]) {
      const context = await read(target, '/v1/events/' + segment(event.eventRef) + '/context', 'native_lark_' + region.region + '_agent');
      assertLarkNativeContext(ctx, region, input, event, context.body, [attachment]);
      ctx.assert(context.body.capabilities.inbound_media?.read_tool === true, 'lark_document_read_tool_available');
      await readLarkDocument(ctx, lease, region, input, event, target);
      await readLarkDocument(ctx, lease, region, input, event, target, 2);
    }
    const delivery = await call('waitForDelivery', {eventId}); assertLarkNativeDelivery(ctx, lease, region, input, event, delivery);
    const document = {region: region.region, input, event, turn, delivery}; turns.push(document); documents.push(document);
    await drain();
    const beforeRefusals = await resources(); assertLarkResourceCalls(ctx, lease, documents, beforeRefusals);
    const path = '/v1/events/' + segment(event.eventRef) + '/attachments/0?view=text&page=1';
    for (const target of [replica, peer(replica)]) {
      for (const actor of ['relay_' + region.region + '_agent', 'native_lark_' + (region.region === 'lark' ? 'feishu' : 'lark') + '_agent']) {
        const refused = await read(target, path, actor, [403]);
        ctx.assert(refused.body?.code === 'forbidden' && refused.body.text === undefined && refused.body.download === undefined,
          'lark_document_foreign_consumer_refused');
      }
      const missing = await read(target, '/v1/events/' + segment(event.eventRef) + '/attachments/1?view=text&page=1',
        'native_lark_' + region.region + '_agent', [404]);
      ctx.assert(missing.body?.code === 'attachment_not_found' && missing.body.text === undefined,
        'lark_document_only_authenticated_attachment_slot');
    }
    await drain(); ctx.assert(same(await resources(), beforeRefusals), 'lark_document_refusals_make_no_resource_calls');
    await ctx.observe('lark_document_completed', async () => ({region: region.region, event_sha256: digest(eventId),
      wire_sha256: wire.artifact_sha256, capture_sha256: digest(captured), provider_sha256: digest(provider),
      content_sha256: digest(input.document.content), delivery_sha256: digest(delivery)}));
  }
  ctx.assert(turns.length === 6 && new Set(turns.map(t => t.event.responseId)).size === 6, 'lark_document_six_distinct_responses');
  for (const region of lease.regions) {
    const own = turns.filter(t => t.region === region.region);
    ctx.assert(own.length === 3 && new Set(own.map(t => t.event.conversationId)).size === 1
      && new Set(own.map(t => t.event.agentId)).size === 1, 'lark_document_keeps_original_native_conversation');
    for (const replica of [ctx.am, peer(ctx.am)]) {
      const native = await read(replica, '/v1/channels/' + segment(region.native.channelId) + '/activity');
      assertLarkNativeActivity(ctx, region, native.body, turns);
      const relay = await read(replica, '/v1/channels/' + segment(region.relay.channelId) + '/activity');
      assertActivityPage(ctx, relay.body, []);
    }
  }
  const effects = await call('captureEffects');
  ctx.assert(Array.isArray(effects) && effects.length === 6 && new Set(effects.map(e => e.platformRequestId)).size === 6
    && turns.every(t => effects.filter(e => e.eventId === t.event.eventId && e.responseId === t.event.responseId
      && e.platformRequestId === t.delivery.platformReceipt.request_id && e.kind === 'native_text'
      && e.artifact_sha256 === t.delivery.platformReceipt.artifact_sha256).length === 1), 'lark_document_no_extra_or_relay_effects');
  await readLarkSources(ctx, lease, initial); await drain();
  const capturedResources = await resources(); assertLarkResourceCalls(ctx, lease, documents, capturedResources);
  await ctx.observe('lark_document_finality', async () => ({turns: 6, deliveries: 6, documents: 2,
    resource_capture_sha256: digest(capturedResources), effects_sha256: digest(effects), relay_resource_access_verified: false}));
  return turns;
}
