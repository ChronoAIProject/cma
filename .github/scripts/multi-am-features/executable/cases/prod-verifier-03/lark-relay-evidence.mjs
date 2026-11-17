import {Blocked} from '../../runner.mjs';
import {digest, same, segment} from './helpers.mjs';

const text = value => typeof value === 'string' && value.length > 0;
const hash = value => /^[a-f0-9]{64}$/.test(value ?? '');
const official = region => region === 'lark' ? 'https://open.larksuite.com' : 'https://open.feishu.cn';
const slug = region => region === 'lark' ? 'api-lark-bot' : 'api-feishu-bot';

export function larkProxyPath(region, suffix, kind) {
  const b = region.relay.binding;
  return '/api/v1/proxy/' + segment(b.catalog_service_id) + suffix
    + '?' + (kind ? 'type=' + kind + '&' : '') + '_nyxid_via=' + segment(b.user_service_id);
}

export function validateLarkRelayRuntime(lease) {
  const runtime = lease.relayRuntime;
  let origin; try {origin = new URL(runtime?.origin);} catch {}
  if (!origin || !['http:', 'https:'].includes(origin.protocol) || !['127.0.0.1', '[::1]'].includes(origin.hostname)
    || origin.username || origin.password || origin.pathname !== '/' || origin.search || origin.hash
    || runtime.binding?.schema !== 'multi-eg-lark-relay-local-binding-v1' || runtime.binding.source !== lease.source
    || runtime.binding.image !== lease.image || !hash(runtime.binding.artifact_sha256)
    || runtime.binding.origin !== runtime.origin || runtime.replyMode !== 'plain')
    throw new Blocked('lark_relay_supported_local_nyxid_plain_reply_binding_required');
  for (const region of lease.regions) if (!hash(region.relay.sourceRevision) || !hash(region.relay.channelKeySha256))
    throw new Blocked('lark_relay_source_revision_and_credential_custody_required');
}

export function assertLarkBrokerCall(ctx, lease, region, call, method, path) {
  ctx.assert(call && text(call.request_id) && hash(call.artifact_sha256) && call.provenance === 'receiver'
    && call.origin === lease.relayRuntime.origin && call.source === lease.source && call.image === lease.image
    && call.method === method && call.path === path && call.status === 200
    && call.credential_sha256 === region.relay.channelKeySha256,
  'lark_relay_actual_scoped_broker_call');
}

// Metadata establishes credential scope only when the actual selected proxy
// bot identity has also been read. Discovery rows alone never authorize bytes.
export function assertLarkProxyAuthority(ctx, lease, region, authority) {
  const binding = region.relay.binding;
  ctx.assert(authority?.schema === 'multi-eg-lark-proxy-authority-v1' && hash(authority.artifact_sha256)
    && Array.isArray(authority.calls) && authority.calls.length === 4
    && new Set(authority.calls.map(c => c.request_id)).size === 4, 'lark_relay_independent_proxy_authority');
  const paths = ['/api/v1/user-services', '/api/v1/endpoints', '/api/v1/services/' + segment(binding.catalog_service_id),
    larkProxyPath(region, '/open-apis/bot/v3/info')];
  const calls = paths.map(path => {
    const matches = authority.calls.filter(c => c.path === path);
    ctx.assert(matches.length === 1, 'lark_relay_exact_authority_read_set');
    assertLarkBrokerCall(ctx, lease, region, matches[0], 'GET', path); return matches[0];
  });
  const selected = calls[0].response?.services?.filter(s => s.id === binding.user_service_id);
  ctx.assert(selected?.length === 1, 'lark_relay_selected_connection_unique');
  const service = selected[0], catalog = calls[2].response;
  ctx.assert(service.slug === slug(region.region) && service.catalog_service_id === binding.catalog_service_id
    && service.is_active === true && service.auth_method === 'token_exchange' && service.node_id == null
    && text(service.endpoint_id) && (service.credential_source?.type !== 'org'
      || service.credential_source.allowed === true && ['admin', 'member'].includes(service.credential_source.role))
    && catalog?.id === binding.catalog_service_id && catalog.slug === slug(region.region)
    && catalog.base_url?.replace(/\/$/, '') === official(region.region)
    && catalog.is_active === true && catalog.auth_method === 'token_exchange', 'lark_relay_selected_current_service_and_catalog');
  const endpoints = calls[1].response?.endpoints?.filter(e => e.id === service.endpoint_id);
  ctx.assert(endpoints?.length === 1 && endpoints[0].catalog_service_id === binding.catalog_service_id
    && (endpoints[0].url?.replace(/\/$/, '') === official(region.region)
      || endpoints[0].auto_connected === true && endpoints[0].url == null), 'lark_relay_selected_regional_endpoint');
  ctx.assert(calls[3].response?.code === 0 && calls[3].response.bot?.open_id === region.relay.botId,
    'lark_relay_selected_connection_bot_custody');
}

export function larkRelayAttachment(region, input) {
  return {reference: 'lark:' + input.resource.key, kind: 'image', variant: 'embedded',
    source: {type: 'lark_resource', message_id: input.platformMessageId, resource_key: input.resource.key,
      resource_kind: 'image', binding: region.relay.binding, bot_id: region.relay.botId, source_revision: region.relay.sourceRevision}};
}

export function assertLarkRelayContext(ctx, region, input, event, body) {
  ctx.assert(body?.activity_verified === true && body.event_id === event.eventId && body.channel_id === region.relay.channelId
    && body.conversation_id === event.conversationId && body.consumer?.id === 'default'
    && same(body.source, {type: 'nyxid_relay', platform: region.region, route_id: region.relay.routeId})
    && body.activity?.actor?.id === region.relay.senderId && body.activity.actor.kind === 'human'
    && same(body.activity.conversation, {id: region.relay.chatId, kind: 'private'})
    && body.activity.kind?.type === 'message' && body.activity.kind.text === input.text
    && body.activity.kind.mentions_bot === false && same(body.activity.kind.attachments, [larkRelayAttachment(region, input)])
    && body.capabilities?.inbound_media?.read_tool === true, 'lark_relay_exact_original_resource_context');
}

export function assertLarkRelayResourceCalls(ctx, lease, admitted, calls) {
  ctx.assert(Array.isArray(calls) && calls.length >= admitted.length
    && new Set(calls.map(c => c.request_id)).size === calls.length, 'lark_relay_actual_resource_call_inventory');
  const seen = new Set();
  for (const call of calls) {
    const item = admitted.find(t => call.path === larkProxyPath(t.region,
      '/open-apis/im/v1/messages/' + t.input.platformMessageId + '/resources/' + t.input.resource.key, 'image'));
    ctx.assert(item, 'lark_relay_no_unselected_or_unscoped_resource_request');
    assertLarkBrokerCall(ctx, lease, item.region, call, 'GET', call.path);
    assertLarkProxyAuthority(ctx, lease, item.region, call.authorization);
    ctx.assert(call.schema === 'multi-eg-lark-relay-resource-v1' && call.content_type === 'image/png'
      && call.body_sha256 === digest(item.input.resource.bytes) && call.bytes === item.input.resource.bytes.length
      && !call.authorization.calls.some(c => c.request_id === call.request_id), 'lark_relay_exact_original_resource_bytes');
    seen.add(item.event.eventId);
  }
  ctx.assert(seen.size === admitted.length, 'lark_relay_each_selected_original_fetched');
}

export function assertLarkRelayMessage(ctx, region, event, reply, delivery, sealed) {
  ctx.assert(reply?.delivery?.outcome === 'confirmed' && text(reply.message_ref)
    && /^om_[A-Za-z0-9_-]+$/.test(reply.delivery.platform_message_id ?? ''), 'lark_relay_confirmed_public_message_reference');
  // The broker's outbound ID and the native om_* ID are different namespaces.
  // Join each to its independently captured reply field before checking the seal.
  const authority = sealed?.authority, outbound = delivery?.exchange?.response;
  ctx.assert(sealed?.schema === 'multi-eg-lark-message-authority-v1' && sealed.seal_verified === true
    && hash(sealed.artifact_sha256) && sealed.message_ref_sha256 === digest(reply.message_ref)
    && sealed.event_id === event.eventId && sealed.channel_id === region.relay.channelId
    && sealed.consumer_id === 'default' && sealed.platform_message_id === reply.delivery.platform_message_id
    && outbound?.platform_message_id === reply.delivery.platform_message_id && text(outbound.message_id)
    && authority?.platform === region.region && authority.bot_id === region.relay.botId
    && authority.chat_id === region.relay.chatId && authority.thread_id == null
    && authority.outbound_message_id === outbound.message_id
    && same(authority.binding, {connection: region.relay.binding, delete: true, reaction: true})
    && hash(authority.authority_sha256) && Number.isSafeInteger(authority.issued_at) && authority.issued_at > 0
    && Number.isSafeInteger(authority.expires_at) && authority.expires_at > authority.issued_at
    && authority.expires_at - authority.issued_at <= 86400,
  'lark_relay_sealed_exact_selected_message_authority');
}

export function assertLarkEffectCapture(ctx, lease, region, messageId, action, capture) {
  ctx.assert(['reaction_set', 'reaction_clear', 'delete'].includes(action)
    && capture?.schema === 'multi-eg-lark-message-effect-v1' && capture.action === action,
  'lark_relay_message_effect_capture');
  const suffix = '/open-apis/im/v1/messages/' + segment(messageId);
  const method = action === 'reaction_set' ? 'POST' : 'DELETE';
  const path = action === 'delete' ? larkProxyPath(region, suffix)
    : larkProxyPath(region, suffix + '/reactions' + (action === 'reaction_clear' ? '/' + segment(capture.reactionId) : ''));
  assertLarkBrokerCall(ctx, lease, region, capture, method, path);
  assertLarkProxyAuthority(ctx, lease, region, capture.authorization);
  const checked = capture.messageRead;
  assertLarkBrokerCall(ctx, lease, region, checked, 'GET', larkProxyPath(region, suffix));
  const rows = checked.response?.data?.items;
  ctx.assert(checked.response?.code === 0 && rows?.length === 1 && rows[0].message_id === messageId
    && rows[0].chat_id === region.relay.chatId && [undefined, ''].includes(rows[0].thread_id)
    && rows[0].deleted !== true && rows[0].sender?.sender_type === 'app'
    && (rows[0].sender.id_type === 'app_id' && rows[0].sender.id === region.relay.binding.app_id
      || rows[0].sender.open_bot_id === region.relay.botId), 'lark_relay_effect_checks_actual_message_sender_and_chat');
  ctx.assert(new Set([...capture.authorization.calls.map(c => c.request_id), checked.request_id, capture.request_id]).size === 6
    && same(capture.request, action === 'reaction_set' ? {reaction_type: {emoji_type: 'THUMBSUP'}} : null)
    && capture.response?.code === 0, 'lark_relay_exact_selected_effect_request');
  if (action !== 'delete') {
    const data = capture.response.data;
    ctx.assert(text(capture.reactionId) && data?.reaction_id === capture.reactionId
      && data.operator?.operator_type === 'app' && [region.relay.binding.app_id, region.relay.botId].includes(data.operator.operator_id)
      && data.reaction_type?.emoji_type === 'THUMBSUP', 'lark_relay_exact_reaction_receipt');
  }
}

// Retain existing credentials and routing. Public source fields are not keys.
export function larkRelayUpdate(record, enabled) {
  const d = record.definition, sources = structuredClone(d.sources);
  if (!enabled) {delete sources[0].lark_resources; delete sources[0].lark_message_effects;}
  return {schema_version: d.schema_version, name: d.name, profile: d.profile,
    provider: {slug: record.provider.slug, ...(d.provider.target ? {target: d.provider.target} : {})},
    sources, partition: d.partition, reply: d.reply, expected_version: record.version};
}

export function assertLarkRelaySaved(ctx, original, saved, enabled, increments) {
  const expected = structuredClone(original);
  expected.version += increments; expected.definition.version = expected.version;
  if (!enabled) {delete expected.definition.sources[0].lark_resources; delete expected.definition.sources[0].lark_message_effects;}
  ctx.assert(same(saved, expected), 'lark_relay_exact_versioned_source_update');
}
