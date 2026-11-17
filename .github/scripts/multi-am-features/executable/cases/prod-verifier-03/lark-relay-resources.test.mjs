import test from 'node:test';
import assert from 'node:assert/strict';
import {createServer} from 'node:http';
import {once} from 'node:events';
import {randomBytes, randomUUID} from 'node:crypto';
import {inflateSync} from 'node:zlib';
import {Blocked} from '../../runner.mjs';
import {digest} from './helpers.mjs';
import {createEgTransport} from './setup.mjs';
import {larkProxyPath, validateLarkRelayRuntime, assertLarkProxyAuthority, larkRelayAttachment, assertLarkRelayContext,
  assertLarkRelayResourceCalls, assertLarkRelayMessage, assertLarkEffectCapture, larkRelayUpdate, assertLarkRelaySaved} from './lark-relay-evidence.mjs';
import {validateLarkRelayFixture, larkRelayInput, assertLarkRelayOriginal, assertLarkRelayDelivery,
  assertLarkRelayImage, readLarkRelayImage, runLarkRelayResources, verifyLarkRelayResult} from './lark-relay-resources.mjs';

const source = 'a'.repeat(40), image = 'fixture@sha256:' + 'b'.repeat(64);
const ctx = {am: 'a', assert: (value, code) => assert.ok(value, code), observe: async (_, read) => read()};
const official = region => region === 'lark' ? 'https://open.larksuite.com' : 'https://open.feishu.cn';
const slug = region => region === 'lark' ? 'api-lark-bot' : 'api-feishu-bot';
function fixture() {
  const origin = 'http://127.0.0.1:12345';
  const lease = {source, image, local: true, synthetic: true, ownerSubject: 'owner',
    counters: async () => {}, close: async () => {}, request: async () => {},
    relayRuntime: {origin, replyMode: 'plain', binding: {
    schema: 'multi-eg-lark-relay-local-binding-v1', source, image, origin, artifact_sha256: digest('binding')}},
    regions: ['lark', 'feishu'].map(region => ({region,
      native: {channelId: region + '-native-channel', recordId: region + '-native-record',
        senderId: 'ou_' + region + '_native_sender', chatId: 'oc_' + region + '_native_chat',
        identity: {region, app_id: 'cli_' + region + '_native_app', tenant_key: region + '_tenant', bot_open_id: 'ou_' + region + '_native_bot'}},
      relay: {channelId: region + '-relay-channel', recordId: region + '-record', senderId: 'ou_' + region + '_sender',
        chatId: 'oc_' + region + '_chat', botId: 'ou_' + region + '_bot', issuer: 'issuer', keyId: 'key', routeId: region + '-route',
        sourceRevision: digest(region), channelKeySha256: digest(randomBytes(32)),
        binding: {user_service_id: region + '-selected', catalog_service_id: region + '-catalog', app_id: 'cli_' + region + '_app'}},
      connections: ['selected', 'unselected'].map(selection => ({user_service_id: region + '-' + selection,
        catalog_service_id: region + '-catalog', name: 'Synthetic ' + selection, platform: region, source: 'personal'}))})),
    ...Object.fromEntries(['prepareRelayResource', 'platformSend', 'callbackCapture', 'relayEventCapture', 'waitForTurn',
      'providerContext', 'waitForDelivery', 'relayReplyCapture', 'relayMessageCapture', 'captureRelayResources',
      'captureRelayMessageEffects', 'captureEffects', 'drain'].map(k => [k, async () => {}]))};
  const initial = {records: new Map(lease.regions.flatMap(r => ['native', 'relay'].map(kind => [r[kind].channelId, record(r, kind)]))),
    capabilities: {relay_authoring: {native_lark_writes_enabled: true, native_lark_intake_enabled: true,
      lark_resource_writes_enabled: true, lark_message_effects_writes_enabled: true, multi_consumer_writes_enabled: true,
      native_lark_controls_enabled: false, native_lark_forms_enabled: false,
      platforms: ['lark', 'feishu'].map(id => ({id, verification_modes: [{id: 'native_event', writes_enabled: true}]}))}}};
  return {lease, initial};
}
function record(region, kind = 'relay') {
  const seed = region[kind], admission = {type: 'scoped', senders: {type: 'allowlist', ids: [seed.senderId]},
    chats: {type: 'allowlist', ids: [seed.chatId]}, groups: 'mention'};
  return {channel_id: seed.channelId, record_id: seed.recordId, version: 3, provider: {slug: 'cma_codex'},
    endpoints: {native_lark_webhook_url: 'https://synthetic.invalid/v1/native-lark/' + seed.channelId},
    definition: {version: 3, schema_version: 1, name: 'Synthetic relay', profile: {metadata: {owner: {subject: 'owner'}}},
      owner: {subject: 'owner'}, creation: {ready: true}, provider: {slug: 'cma_codex'}, secrets: {agent_key_hash: digest('agent')},
      sources: [kind === 'native' ? {type: 'native_lark', separate_app: true, identity: seed.identity, admission}
        : {type: 'nyxid_relay', platform: region.region, issuer: seed.issuer, key_id: seed.keyId,
          route_ids: [seed.routeId], bot_id: seed.botId, admission, lark_resources: seed.binding,
          lark_message_effects: {connection: seed.binding, delete: true, reaction: true}}],
      partition: {version: 1, key: 'conversation_and_sender'}, reply: {auto_final: true, ack_after_ms: 0, sinks: [{type: 'source'}]},
      routing: {version: 1, default_targets: ['default']}}};
}
function broker(lease, region, method, path, response) {
  return {request_id: randomUUID(), artifact_sha256: digest(randomUUID()), provenance: 'receiver', origin: lease.relayRuntime.origin,
    source, image, method, path, status: 200, credential_sha256: region.relay.channelKeySha256, response};
}
function authority(lease, region) {
  const b = region.relay.binding, endpointId = region.region + '-endpoint';
  return {schema: 'multi-eg-lark-proxy-authority-v1', artifact_sha256: digest('authority'), calls: [
    broker(lease, region, 'GET', '/api/v1/endpoints', {endpoints: [{id: endpointId, catalog_service_id: b.catalog_service_id, url: official(region.region)}]}),
    broker(lease, region, 'GET', '/api/v1/user-services', {services: [{id: b.user_service_id, slug: slug(region.region),
      catalog_service_id: b.catalog_service_id, endpoint_id: endpointId, is_active: true, auth_method: 'token_exchange'}]}),
    broker(lease, region, 'GET', '/api/v1/services/' + b.catalog_service_id, {id: b.catalog_service_id, slug: slug(region.region),
      base_url: official(region.region), is_active: true, auth_method: 'token_exchange'}),
    broker(lease, region, 'GET', larkProxyPath(region, '/open-apis/bot/v3/info'), {code: 0, bot: {open_id: region.relay.botId}})]};
}
function admitted(region) {
  const input = larkRelayInput(region, 'a');
  return {region, input, event: {eventId: input.messageId, eventRef: 'sealed-' + region.region, consumerId: 'default',
    conversationId: region.region + '-conversation', agentId: region.region + '-agent', responseId: region.region + '-response'}};
}
function resource(lease, item) {
  return {...broker(lease, item.region, 'GET', larkProxyPath(item.region,
    '/open-apis/im/v1/messages/' + item.input.platformMessageId + '/resources/' + item.input.resource.key, 'image')),
    schema: 'multi-eg-lark-relay-resource-v1', content_type: 'image/png', bytes: item.input.resource.bytes.length,
    body_sha256: digest(item.input.resource.bytes), authorization: authority(lease, item.region)};
}
function effect(lease, region, action, messageId = 'om_outbound') {
  const suffix = '/open-apis/im/v1/messages/' + messageId, reactionId = 'reaction-one';
  const path = action === 'delete' ? larkProxyPath(region, suffix)
    : larkProxyPath(region, suffix + '/reactions' + (action === 'reaction_clear' ? '/' + reactionId : ''));
  return {...broker(lease, region, action === 'reaction_set' ? 'POST' : 'DELETE', path,
    action === 'delete' ? {code: 0} : {code: 0, data: {reaction_id: reactionId,
      operator: {operator_id: region.relay.binding.app_id, operator_type: 'app'}, reaction_type: {emoji_type: 'THUMBSUP'}}}),
    schema: 'multi-eg-lark-message-effect-v1', action, reactionId, authorization: authority(lease, region),
    request: action === 'reaction_set' ? {reaction_type: {emoji_type: 'THUMBSUP'}} : null,
    messageRead: broker(lease, region, 'GET', larkProxyPath(region, suffix), {code: 0, data: {items: [{message_id: messageId,
      chat_id: region.relay.chatId, sender: {sender_type: 'app', id_type: 'app_id', id: region.relay.binding.app_id}}]}})};
}

test('relay resources require actual local broker binding and low-level primitives before dispatch', async () => {
  const f = fixture(); validateLarkRelayFixture(f.lease, f.initial);
  for (const edit of [f => f.lease.relayRuntime.origin = 'https://nyxid.example', f => f.lease.relayRuntime.replyMode = 'unknown',
    f => f.lease.relayRuntime.binding.source = 'c'.repeat(40), f => delete f.lease.captureRelayResources,
    f => delete f.lease.relayMessageCapture, f => f.lease.regions[0].relay.sourceRevision = '',
    f => f.initial.records.get(f.lease.regions[0].relay.channelId).definition.routing.default_targets.push('extra')]) {
    const next = fixture(); edit(next); let calls = 0; next.lease.platformSend = async () => {calls++;};
    await assert.rejects(runLarkRelayResources(ctx, next.lease, next.initial), error => error instanceof Blocked);
    assert.equal(calls, 0);
  }
  assert.throws(() => validateLarkRelayRuntime({}), error => error instanceof Blocked);
});

test('relay rich posts preserve native raw facts while normalized content remains unknown', () => {
  const region = fixture().lease.regions[0], input = larkRelayInput(region, 'b');
  assert.deepEqual(input.normalizedContent, {type: 'unknown', attachments: []});
  assert.equal(input.rawEvent.event.message.message_type, 'post');
  assert.deepEqual(JSON.parse(input.rawEvent.event.message.content), {title: '', content: [[
    {tag: 'text', text: input.text}, {tag: 'img', image_key: input.resource.key}]]});
  assert.equal(input.rawEvent.header.app_id, region.relay.binding.app_id);
  assert.equal(input.resource.bytes.subarray(1, 4).toString(), 'PNG');
  assert.equal(input.resource.bytes.readUInt32BE(16), 1); assert.equal(input.resource.bytes.readUInt32BE(20), 1);
  const compressed = []; let offset = 8;
  while (offset < input.resource.bytes.length) {
    const n = input.resource.bytes.readUInt32BE(offset), type = input.resource.bytes.toString('ascii', offset + 4, offset + 8);
    if (type === 'IDAT') compressed.push(input.resource.bytes.subarray(offset + 8, offset + 8 + n));
    offset += n + 12;
  }
  assert.equal(inflateSync(Buffer.concat(compressed)).length, 3);
  assert.notEqual(larkRelayInput(region, 'a').resource.key, input.resource.key);
});

test('selected relay authority joins actual service catalog endpoint and proxied bot identity', () => {
  const {lease} = fixture(), r = lease.regions[0]; assertLarkProxyAuthority(ctx, lease, r, authority(lease, r));
  for (const edit of [a => a.calls[1].response.services[0].id = 'unselected', a => a.calls[1].response.services[0].is_active = false,
    a => a.calls[1].response.services.push({...a.calls[1].response.services[0]}), a => a.calls[1].response.services[0].node_id = 'node',
    a => a.calls[1].response.services[0].credential_source = {type: 'org', role: 'member', allowed: false},
    a => a.calls[0].response.endpoints[0].url = official('feishu'), a => a.calls[2].response.slug = 'generic-proxy',
    a => a.calls[3].response.bot.open_id = 'ou_other', a => a.calls[3].path = a.calls[3].path.replace('selected', 'unselected'),
    a => a.calls[0].credential_sha256 = digest('native-token'), a => a.calls[0].provenance = 'sender', a => a.calls.pop()]) {
    const a = authority(lease, r); edit(a); assert.throws(() => assertLarkProxyAuthority(ctx, lease, r, a), /lark_relay_/);
  }
});

test('resource captures require selected proxy bytes and reject native or unselected authority', () => {
  const {lease} = fixture(), items = lease.regions.map(admitted), calls = items.map(t => resource(lease, t));
  assertLarkRelayResourceCalls(ctx, lease, items, calls);
  for (const edit of [c => c[0].path = c[0].path.replace('selected', 'unselected'), c => c[0].path = '/v1/media/content',
    c => c[0].body_sha256 = digest('wrong'), c => c[0].bytes++, c => c[0].content_type = 'application/json',
    c => c[0].credential_sha256 = digest('native-token'), c => c[0].authorization.calls[3].response.bot.open_id = 'ou_native',
    c => c[0].request_id = c[1].request_id, c => c.push({...c[0], request_id: randomUUID(), path: '/unscoped'}), c => c.pop()]) {
    const c = structuredClone(calls); edit(c); assert.throws(() => assertLarkRelayResourceCalls(ctx, lease, items, c), /lark_relay_/);
  }
});

test('relay event seals and public context retain exact original embedded resource scope', () => {
  const {lease} = fixture(), r = lease.regions[0], t = admitted(r), attachment = larkRelayAttachment(r, t.input);
  const wire = {raw_platform_data: t.input.rawEvent, raw_event_sha256: digest(t.input.rawEvent),
    normalized_content: t.input.normalizedContent, reply_token_sha256: digest(randomBytes(32))};
  const capture = {...t.event, schema: 'multi-eg-lark-relay-event-v1', seal_verified: true, artifact_sha256: digest('event'),
    event_ref_sha256: digest(t.event.eventRef), request_id: 'am-request', source: {type: 'nyxid_relay', platform: r.region, route_id: r.relay.routeId},
    actor_id: r.relay.senderId, text_sha256: digest(t.input.text), attachments: [attachment], reply: {type: 'nyxid_relay',
      platform: r.region, route_id: r.relay.routeId, message_id: t.input.messageId, thread_reply: false, reply_token_sha256: wire.reply_token_sha256}};
  const turn = {amRequests: [{request_id: 'am-request'}]};
  assertLarkRelayOriginal(ctx, r, t.input, t.event, turn, wire, capture);
  for (const edit of [c => c.request_id = 'eg-request', c => c.seal_verified = false,
    c => c.attachments[0].source.binding.user_service_id = 'unselected', c => c.attachments[0].source.source_revision = digest('other'),
    c => c.reply.message_id = t.input.platformMessageId]) {
    const c = structuredClone(capture); edit(c);
    assert.throws(() => assertLarkRelayOriginal(ctx, r, t.input, t.event, turn, wire, c), /lark_relay_/);
  }
  const body = {event_id: t.event.eventId, channel_id: r.relay.channelId, conversation_id: t.event.conversationId,
    activity_verified: true, consumer: {id: 'default'}, source: capture.source,
    activity: {actor: {id: r.relay.senderId, kind: 'human'}, conversation: {id: r.relay.chatId, kind: 'private'},
      kind: {type: 'message', text: t.input.text, mentions_bot: false, attachments: [attachment]}}, capabilities: {inbound_media: {read_tool: true}}};
  assertLarkRelayContext(ctx, r, t.input, t.event, body);
  body.activity.kind.attachments[0].source.type = 'native_lark';
  assert.throws(() => assertLarkRelayContext(ctx, r, t.input, t.event, body), /lark_relay_/);
});

test('relay message effects require independent current connection and confirmed bot message reads', () => {
  const {lease} = fixture(), r = lease.regions[1];
  for (const action of ['reaction_set', 'reaction_clear', 'delete']) {
    assertLarkEffectCapture(ctx, lease, r, 'om_outbound', action, effect(lease, r, action));
    for (const edit of [c => c.path = c.path.replace('selected', 'unselected'), c => c.messageRead.response.data.items[0].chat_id = 'oc_foreign',
      c => c.messageRead.response.data.items[0].sender.id = 'cli_foreign', c => c.messageRead.response.data.items[0].deleted = true,
      c => c.messageRead.response.data.items.push({...c.messageRead.response.data.items[0]}),
      c => c.authorization.calls[3].response.bot.open_id = 'ou_native', c => c.response.code = 99,
      c => c.request_id = c.messageRead.request_id, c => c.request = {connection_ref: 'unselected'}]) {
      const c = effect(lease, r, action); edit(c);
      assert.throws(() => assertLarkEffectCapture(ctx, lease, r, 'om_outbound', action, c), /lark_relay_/);
    }
  }
  const bad = effect(lease, r, 'reaction_set'); bad.response.data.operator.operator_type = 'user';
  assert.throws(() => assertLarkEffectCapture(ctx, lease, r, 'om_outbound', 'reaction_set', bad), /lark_relay_/);
});

test('a sealed relay message binds the public receipt to selected app chat and consumer', () => {
  const {lease} = fixture(), r = lease.regions[0], t = admitted(r), reply = {message_ref: 'sealed-message',
    delivery: {outcome: 'confirmed', platform_message_id: 'om_outbound'}};
  const delivery = {exchange: {response: {message_id: randomUUID(), platform_message_id: 'om_outbound'}}};
  const captured = {schema: 'multi-eg-lark-message-authority-v1', seal_verified: true, artifact_sha256: digest('seal'),
    message_ref_sha256: digest(reply.message_ref), event_id: t.event.eventId, channel_id: r.relay.channelId, consumer_id: 'default',
    platform_message_id: 'om_outbound', authority: {platform: r.region, bot_id: r.relay.botId, chat_id: r.relay.chatId, thread_id: null,
      outbound_message_id: delivery.exchange.response.message_id, binding: {connection: r.relay.binding, delete: true, reaction: true},
      authority_sha256: digest('authority'), issued_at: 1, expires_at: 86401}};
  assertLarkRelayMessage(ctx, r, t.event, reply, delivery, captured);
  for (const edit of [c => c.seal_verified = false, c => c.consumer_id = 'other', c => c.message_ref_sha256 = digest('other'),
    c => c.authority.binding.connection.app_id = 'cli_native', c => c.authority.chat_id = 'oc_other',
    c => c.authority.outbound_message_id = 'om_original', c => c.authority.outbound_message_id = reply.delivery.platform_message_id,
    c => c.authority.issued_at = 0, c => c.authority.expires_at = 1, c => c.authority.expires_at = 86402]) {
    const c = structuredClone(captured); edit(c); assert.throws(() => assertLarkRelayMessage(ctx, r, t.event, reply, delivery, c), /lark_relay_/);
  }
  for (const edit of [d => d.exchange.response.message_id = 'other-relay-id', d => d.exchange.response.platform_message_id = 'om_other']) {
    const d = structuredClone(delivery); edit(d);
    assert.throws(() => assertLarkRelayMessage(ctx, r, t.event, reply, d, captured), /lark_relay_/);
  }
});

test('relay consent updates omit credentials and preserve all other source and revision fields', () => {
  const r = fixture().lease.regions[0], before = record(r), off = larkRelayUpdate(before, false), on = larkRelayUpdate(before, true);
  assert.equal(off.expected_version, 3); assert.equal(off.credentials, undefined); assert.equal(off.routing, undefined);
  assert.equal(off.sources[0].lark_resources, undefined); assert.equal(off.sources[0].lark_message_effects, undefined);
  assert.deepEqual(on.sources, before.definition.sources); assert.deepEqual(off.profile, before.definition.profile);
  const removed = structuredClone(before); removed.version = 4; removed.definition.version = 4; removed.definition.sources = off.sources;
  assertLarkRelaySaved(ctx, before, removed, false, 1);
  const restored = structuredClone(before); restored.version = 5; restored.definition.version = 5;
  assertLarkRelaySaved(ctx, before, restored, true, 2);
  for (const edit of [v => v.definition.version = 3, v => v.definition.secrets.agent_key_hash = digest('changed'),
    v => v.definition.profile.metadata.owner.subject = 'other', v => v.definition.routing.default_targets.push('other')]) {
    const v = structuredClone(restored); edit(v); assert.throws(() => assertLarkRelaySaved(ctx, before, v, true, 2), /lark_relay_/);
  }
});

function imageView() {return {index: 0, kind: 'image', variant: 'embedded', status: 'ready',
  images: [{origin: 'normalized', mime_type: 'image/png', width: 1, height: 1, bytes: 68}]};}
test('relay image views require derived selected bytes rather than a generic ready flag', () => {
  assertLarkRelayImage(ctx, imageView());
  for (const edit of [v => v.images = [], v => v.images[0].width = 2, v => v.images[0].origin = 'platform_thumbnail',
    v => v.images[0].bytes = 0, v => v.status = 'unavailable', v => v.variant = 'native', v => v.download = {}]) {
    const v = imageView(); edit(v); assert.throws(() => assertLarkRelayImage(ctx, v), /lark_relay_/);
  }
});

test('relay delivery needs actual broker exchange plus exact independent platform effect', () => {
  const {lease} = fixture(), r = lease.regions[0], t = admitted(r), value = 'Synthetic relay answer', token = digest(randomBytes(32));
  const captured = {schema: 'multi-eg-lark-relay-delivery-v1', artifact_sha256: digest('delivery'), exchange: {
    ...broker(lease, r, 'POST', '/api/v1/channel-relay/reply', {message_id: 'new-relay-id', platform_message_id: 'om_outbound'}),
    credential_sha256: token, request: {message_id: t.event.eventId, reply: {text_sha256: digest(value), metadata: null}}},
    platform: {provenance: 'receiver', request_id: randomUUID(), artifact_sha256: digest('platform'), platform: r.region,
      message_id: 'om_outbound', chat_id: r.relay.chatId, bot_id: r.relay.botId, app_id: r.relay.binding.app_id,
      text_sha256: digest(value), count: 1, status: 'delivered'}};
  assertLarkRelayDelivery(ctx, lease, r, t.event, value, captured, token);
  for (const edit of [c => c.exchange.path = '/native/send', c => c.exchange.credential_sha256 = digest('other'),
    c => c.exchange.request.reply.text_sha256 = digest('wrong'), c => c.platform.provenance = 'sender',
    c => c.platform.app_id = 'cli_native', c => c.platform.chat_id = 'oc_other', c => c.platform.count = 2,
    c => c.platform.message_id = 'om_different', c => c.platform.status = 'accepted']) {
    const c = structuredClone(captured); edit(c); assert.throws(() => assertLarkRelayDelivery(ctx, lease, r, t.event, value, c, token), /lark_relay_/);
  }
});

test('pending relay image reads use real GETs on both observed EGs with cancellation and no replay', async t => {
  const {lease} = fixture(), region = lease.regions[0], item = admitted(region), members = {}, requests = [];
  const counts = new Map(); let pendingOnly = false;
  for (const slot of ['a', 'b']) {
    const server = createServer((req, res) => {
      requests.push({slot, method: req.method, path: req.url}); const count = counts.get(slot) ?? 0; counts.set(slot, count + 1);
      res.writeHead(200, {'content-type': 'application/json', 'x-request-id': slot + '-' + requests.length});
      res.end(JSON.stringify(pendingOnly || count === 0 ? {index: 0, kind: 'image', variant: 'embedded', status: 'pending',
        retry_after_ms: pendingOnly ? 1000 : 1} : imageView()));
    });
    server.listen(0, '127.0.0.1'); await once(server, 'listening');
    t.after(() => new Promise(resolve => {server.close(resolve); server.closeAllConnections();}));
    members[slot] = {origin: 'http://127.0.0.1:' + server.address().port, instance: 'eg-' + slot, instance_id: 'eg-' + slot,
      pod_uid: 'pod-' + slot, container_id: 'container-' + slot, ready: true, source, image};
  }
  const transport = await createEgTransport({scope: 'LOCAL_DISPOSABLE_TWO_AM', synthetic: true, shared_database: true,
    expectedSource: source, expectedImage: image, topology: async () => members, authorize: async () => ({}),
    witness: async ({replica, responseRequestId}) => ({slot: replica, instance: 'eg-' + replica, requestId: responseRequestId})});
  lease.request = options => transport.request(options); lease.replicas = transport.replicas;
  for (const replica of ['a', 'b']) await readLarkRelayImage(ctx, lease, region, item.event, replica);
  assert.equal(requests.length, 4); assert.ok(requests.every(r => r.method === 'GET' && r.path.endsWith('/attachments/0?view=image')));
  pendingOnly = true; const controller = new AbortController(), request = lease.request;
  lease.request = async options => {const result = await request(options); controller.abort(); return result;};
  await assert.rejects(readLarkRelayImage({...ctx, signal: controller.signal}, lease, region, item.event, 'a'),
    error => error.name === 'AbortError');
  assert.equal(requests.length, 5);
});

// Full relay orchestration against stateful protocol doubles. HTTP receipts are
// from these loopback servers; seals, AM/provider history and broker/platform
// captures are synthetic test data. This is not EG, AM or compiled-UI execution.
async function orchestration(t, fault) {
  const {lease, initial} = fixture(), stored = structuredClone(initial.records);
  const events = new Map(), messages = new Map(), wire = new Map(), replies = new Map(), staged = new Map();
  const calls = [], resourceCalls = [], effectCalls = [], effects = [], observations = [], members = {};
  const stamp = new Date().toISOString();
  const nativeTurns = lease.regions.flatMap(region => Array.from({length: 3}, (_, index) => {
    const event = {eventId: 'native-event-' + region.region + index, responseId: 'native-response-' + region.region + index,
      conversationId: 'native-conversation-' + region.region, agentId: 'native-agent-' + region.region, consumerId: 'default'};
    const delivery = {platformReceipt: {request_id: randomUUID(), artifact_sha256: digest(randomUUID())}};
    effects.push({eventId: event.eventId, responseId: event.responseId, platformRequestId: delivery.platformReceipt.request_id,
      artifact_sha256: delivery.platformReceipt.artifact_sha256, kind: 'native_text'});
    return {region: region.region, event, turn: turn(event, index % 2 ? 'b' : 'a'), delivery,
      input: {event: {message: {chat_type: 'p2p'}}}};
  }));
  function turn(event, slot) {
    const requestId = randomUUID(), owner = slot === 'a' ? 'b' : 'a';
    const serving = {schema: 'c275-serving-v1', am: slot, origin: 'http://127.0.0.1:12345',
      pod_uid: 'am-pod-' + slot, instance_id: 'am-' + slot, container_id: 'am-container-' + slot,
      ready: true, observed_at: stamp};
    return {...event, servingAm: slot, owningAm: owner, epoch: 3, providerAccepted: true, turnCount: 1, effectCount: 1,
      amRequests: [{am: slot, request_id: requestId, elapsed_ms: 0, status: 202, agent_id_sha256: digest(event.agentId),
        serving, serving_after: {...serving}, started_at: stamp, ended_at: stamp,
        attribution: {verdict: 'PASS', basis: 'pod_forward_before_after'}, owner_proof: {verdict: 'PASS', kind: 'non_owner',
          agent_sha256: digest(event.agentId), request_id: requestId, ingress: slot, owner, owner_epoch: 3,
          owner_instance_sha256: digest('am-' + owner), ingress_instance_sha256: digest('am-' + slot)}}]};
  }
  function delivery(region, event, content, token = region.relay.channelKeySha256) {
    const outbound = randomUUID(), messageId = 'om_' + randomUUID().replaceAll('-', '');
    const result = {schema: 'multi-eg-lark-relay-delivery-v1', artifact_sha256: digest(randomUUID()), exchange: {
      ...broker(lease, region, 'POST', '/api/v1/channel-relay/reply', {message_id: outbound, platform_message_id: messageId}),
      credential_sha256: token, request: {message_id: event.eventId, reply: {text_sha256: digest(content), metadata: null}}},
      platform: {provenance: 'receiver', request_id: randomUUID(), artifact_sha256: digest(randomUUID()), platform: region.region,
        message_id: messageId, chat_id: region.relay.chatId, bot_id: region.relay.botId, app_id: region.relay.binding.app_id,
        text_sha256: digest(content), count: 1, status: 'delivered'}};
    effects.push({platformRequestId: result.platform.request_id}); return result;
  }
  function activity(region, kind) {
    const turns = kind === 'native' ? nativeTurns.filter(v => v.region === region.region)
      : [...events.values()].filter(v => v.region === region);
    return {cursor: 'cursor', retention_days: 7, limit: 100, has_more: false, items: turns.map(({event}) => ({
      event_id: event.eventId, received_at: stamp, occurred_at: stamp,
      source: kind === 'native' ? {type: 'native_lark', ...region.native.identity}
        : {type: 'nyxid_relay', platform: region.region, route_id: region.relay.routeId},
      actor: {id: region[kind].senderId, kind: 'human'}, conversation: {id: region[kind].chatId, kind: 'private'},
      consumers: [{consumer_id: 'default', state: 'completed', agent_id: event.agentId, response_id: event.responseId,
        conversation_id: event.conversationId, dispatched_at: stamp, accepted_at: stamp, produced_at: stamp, completed_at: stamp,
        deliveries: [{step: 'answer', outcome: 'confirmed', observed_at: stamp, confirmed_at: stamp}]}]}))};
  }
  const denied = detail => ({status: 403, body: {code: 'forbidden', ...(detail ? {detail} : {})}});
  async function route(slot, req, body, requestId) {
    const url = new URL(req.url, 'http://127.0.0.1'), actor = req.headers['x-test-actor'];
    if (url.pathname === '/v1/channel-capabilities') return {body: initial.capabilities};
    if (url.pathname === '/v1/relay-resource-connections')
      return {body: {items: lease.regions.find(r => r.region === url.searchParams.get('platform')).connections}};
    for (const region of lease.regions) for (const kind of ['native', 'relay']) {
      const seed = region[kind], path = '/v1/channels/' + seed.channelId;
      if (url.pathname === path + '/activity') return {body: activity(region, kind)};
      if (url.pathname === path) {
        assert.equal(actor, 'owner');
        const saved = stored.get(seed.channelId);
        if (req.method === 'PUT') {
          assert.equal(body.expected_version, saved.version);
          assert.equal(body.credentials, undefined); assert.equal(body.routing, undefined);
          saved.version++; saved.definition.version = saved.version;
          saved.definition.sources = body.sources;
        } else assert.equal(req.method, 'GET');
        return {body: saved};
      }
    }
    if (url.pathname.startsWith('/v1/callbacks/')) {
      const region = lease.regions.find(r => url.pathname.endsWith('/' + r.relay.channelId));
      const input = staged.get(body.messageId);
      assert.deepEqual(body.rawEvent, input.rawEvent); assert.deepEqual(body.normalizedContent, {type: 'unknown', attachments: []});
      const event = {eventId: input.messageId, responseId: randomUUID(), conversationId: 'relay-conversation-' + region.region,
        agentId: 'relay-agent-' + region.region, consumerId: 'default', eventRef: 'sealed-' + input.messageId,
        destination: region.relay.chatId, expectedSender: region.relay.botId, expectedContentDigest: digest(input.completionText)};
      const evidence = turn(event, slot), token = digest(randomBytes(32)), attachment = {
        reference: 'lark:' + input.resource.key, kind: 'image', variant: 'embedded', source: {type: 'lark_resource',
          message_id: input.platformMessageId, resource_key: input.resource.key, resource_kind: 'image',
          binding: region.relay.binding, bot_id: region.relay.botId, source_revision: region.relay.sourceRevision}};
      const captured = {...event, schema: 'multi-eg-lark-relay-event-v1', seal_verified: true, artifact_sha256: digest(randomUUID()),
        request_id: evidence.amRequests[0].request_id, event_ref_sha256: digest(event.eventRef),
        source: {type: 'nyxid_relay', platform: region.region, route_id: region.relay.routeId},
        actor_id: region.relay.senderId, text_sha256: digest(input.text), attachments: [attachment],
        reply: {type: 'nyxid_relay', platform: region.region, route_id: region.relay.routeId,
          message_id: input.messageId, thread_reply: false, reply_token_sha256: token}};
      wire.set(requestId, {schema: 'multi-eg-relay-wire-v1', request_id: requestId, method: 'POST', path: url.pathname,
        ...Object.fromEntries(['artifact_sha256', 'body_sha256', 'callback_body_sha256', 'signed_headers_sha256',
          'callback_token_sha256'].map(k => [k, digest('test-callback')])), reply_token_sha256: token, factsBasis: 'raw_event',
        facts: {platform: region.region, issuer: region.relay.issuer, keyId: region.relay.keyId, messageId: input.messageId,
          correlationId: input.correlationId, routeId: input.routeId, senderId: input.senderId, chatId: input.chatId,
          isGroup: false, mentioned: false, isBotSender: false, textDigest: digest(input.text)},
        raw_platform_data: input.rawEvent, raw_event_sha256: digest(input.rawEvent), normalized_content: input.normalizedContent});
      events.set(event.eventId, {region, input, event, evidence, captured, delivery: {
        eventId: event.eventId, responseId: event.responseId, destination: region.relay.chatId, sender: region.relay.botId,
        contentDigest: event.expectedContentDigest, count: 1, status: 'delivered',
        platformReceipt: delivery(region, event, input.completionText, token)}});
      return {status: 202, body: {status: 'accepted', event_id: event.eventId, conversation_id: event.conversationId}};
    }
    const pieces = url.pathname.split('/'), eventItem = [...events.values()].find(v => v.event.eventRef === pieces[3]);
    if (pieces[2] === 'events' && eventItem) {
      const {region, input, event, captured} = eventItem;
      if (actor !== 'relay_' + region.region + '_agent') return denied();
      if (pieces[4] === 'context') return {body: {event_id: event.eventId, channel_id: region.relay.channelId,
        conversation_id: event.conversationId, activity_verified: true, consumer: {id: 'default'}, source: captured.source,
        activity: {actor: {id: region.relay.senderId, kind: 'human'}, conversation: {id: region.relay.chatId, kind: 'private'},
          kind: {type: 'message', text: input.text, mentions_bot: false, attachments: captured.attachments}},
        capabilities: {inbound_media: {read_tool: true}}}};
      if (pieces[4] === 'attachments') {
        if (!stored.get(region.relay.channelId).definition.sources[0].lark_resources) return denied('attachment_source_revoked');
        if (!eventItem.cached) {resourceCalls.push(resource(lease, eventItem)); eventItem.cached = true;}
        return {body: imageView()};
      }
      if (pieces[4] === 'replies') {
        assert.equal(req.method, 'POST'); assert.ok(req.headers['idempotency-key']);
        const capture = delivery(region, event, body.text), messageRef = 'message-' + randomUUID();
        const messageId = capture.exchange.response.platform_message_id;
        const sealed = {schema: 'multi-eg-lark-message-authority-v1', seal_verified: true, artifact_sha256: digest(randomUUID()),
          request_id: requestId, message_ref_sha256: digest(messageRef), event_id: event.eventId, channel_id: region.relay.channelId,
          consumer_id: 'default', platform_message_id: messageId, authority: {platform: region.region, bot_id: region.relay.botId,
            chat_id: region.relay.chatId, thread_id: null, outbound_message_id: capture.exchange.response.message_id,
            binding: {connection: region.relay.binding, delete: true, reaction: true}, authority_sha256: digest(randomUUID()),
            issued_at: 1, expires_at: 86401}};
        messages.set(messageRef, {region, messageId, sealed, intents: new Map(), deleted: false}); replies.set(requestId, capture);
        return {body: {message_ref: messageRef, delivery: {outcome: 'confirmed', platform_message_id: messageId}}};
      }
    }
    const message = messages.get(pieces[3]);
    if (pieces[2] === 'messages' && message) {
      const {region, messageId, sealed, intents} = message;
      if (actor !== 'relay_' + region.region + '_agent') return denied();
      if (body?.connection_ref) return {status: 400, body: {code: 'invalid_request'}};
      if (!stored.get(region.relay.channelId).definition.sources[0].lark_message_effects) return denied('message_effect_revoked');
      if (pieces[4] === 'capabilities') return {body: {delete: true, reaction: true, scope: 'confirmed_native_bot_message',
        max_reactions: 1, expires_at: sealed.authority.expires_at}};
      const key = req.headers['idempotency-key']; assert.ok(key);
      if (intents.has(key)) return {body: intents.get(key)};
      assert.equal(req.method, 'POST'); assert.equal(message.deleted, false);
      const action = pieces[4] === 'delete' ? 'delete' : body.reaction.length ? 'reaction_set' : 'reaction_clear';
      const capture = {...effect(lease, region, action, messageId), eg_request_id: requestId, operation_key_sha256: digest(key)};
      effectCalls.push(capture); effects.push({platformRequestId: capture.request_id});
      if (action === 'delete') message.deleted = true;
      const result = {delivery: fault === 'unknown_effect' ? {outcome: 'unknown', reason: 'lark_effect_unconfirmed'}
        : {outcome: 'confirmed', platform_message_id: messageId}};
      intents.set(key, result); return {body: result};
    }
    throw Error('unexpected_protocol_route');
  }
  for (const slot of ['a', 'b']) {
    const server = createServer(async (req, res) => {
      const requestId = randomUUID(), chunks = []; for await (const chunk of req) chunks.push(chunk);
      const body = chunks.length ? JSON.parse(Buffer.concat(chunks).toString()) : undefined;
      calls.push({slot, method: req.method, path: req.url, body, key: req.headers['idempotency-key']});
      try {
        const result = await route(slot, req, body, requestId);
        res.writeHead(result.status ?? 200, {'content-type': 'application/json', 'x-request-id': requestId});
        res.end(JSON.stringify(result.body));
      } catch (error) {res.writeHead(500, {'content-type': 'application/json'}); res.end(JSON.stringify({error: error.message}));}
    });
    server.listen(0, '127.0.0.1'); await once(server, 'listening');
    t.after(() => new Promise(resolve => {server.close(resolve); server.closeAllConnections();}));
    members[slot] = {origin: 'http://127.0.0.1:' + server.address().port, instance: 'eg-' + slot, instance_id: 'eg-' + slot,
      pod_uid: 'eg-pod-' + slot, container_id: 'eg-container-' + slot, ready: true, source, image};
  }
  const transport = await createEgTransport({scope: 'LOCAL_DISPOSABLE_TWO_AM', synthetic: true, shared_database: true,
    expectedSource: source, expectedImage: image, topology: async () => members,
    authorize: async ({actor}) => ({'x-test-actor': actor}),
    witness: async ({replica, responseRequestId}) => ({slot: replica, instance: 'eg-' + replica, requestId: responseRequestId})});
  Object.assign(lease, transport, {
    prepareRelayResource: async input => {assert.ok(input.bytes.length > 0);},
    platformSend: async input => {
      staged.set(input.messageId, input);
      const response = await transport.request({method: 'POST', path: '/v1/callbacks/' + input.channelId,
        replica: input.replica, body: {messageId: input.messageId, rawEvent: input.rawEvent, normalizedContent: input.normalizedContent},
        allowed: [202], signal: input.signal, timeoutMs: input.timeoutMs});
      return {...events.get(input.messageId).event, ingress: {receipt: response.eg_receipt, status: response.status,
        body: response.body, contentType: 'application/json', protocolVersion: '1'}};
    },
    callbackCapture: async ({requestId}) => structuredClone(wire.get(requestId)),
    waitForTurn: async ({eventId}) => structuredClone(events.get(eventId).evidence),
    relayEventCapture: async ({eventId}) => structuredClone(events.get(eventId).captured),
    providerContext: async ({eventId}) => {
      const item = events.get(eventId); return {...item.event, requestCount: 1, messages: [{role: 'user', content: item.input.text}]};
    },
    waitForDelivery: async ({eventId}) => structuredClone(events.get(eventId).delivery),
    relayReplyCapture: async ({requestId}) => structuredClone(replies.get(requestId)),
    relayMessageCapture: async ({messageRef}) => structuredClone(messages.get(messageRef).sealed),
    captureRelayResources: async () => structuredClone(resourceCalls),
    captureRelayMessageEffects: async () => structuredClone(effectCalls),
    captureEffects: async () => structuredClone(effects),
    drain: async () => ({turns: 6 + events.size, deliveries: 6 + events.size + replies.size,
      effects: effects.length, messageEffects: effectCalls.length, pending: 0, unknown: 0,
      lateEffects: fault === 'late_effect' && replies.size === 2 ? 1 : 0, mediaPending: 0, mediaUnknown: 0}),
  });
  const context = {...ctx, observe: async (label, read) => {
    const value = await read(); observations.push({label, value}); return value;
  }};
  return {lease, initial, nativeTurns, context, calls, observations, events, replies, effectCalls, effects, stored};
}

test('the complete relay journey preserves two-EG reads, stable effect replay, revocation and restored sources', async t => {
  const f = await orchestration(t);
  const journey = await runLarkRelayResources(f.context, f.lease, f.initial, f.nativeTurns);
  const result = await verifyLarkRelayResult(f.context, f.lease, f.initial, journey);
  assert.equal(result.turns.length, 8); assert.equal(result.effectCalls.length, 6);
  assert.equal(f.effects.length, 16); assert.equal(f.replies.size, 2);
  assert.equal(f.calls.filter(c => c.path.startsWith('/v1/callbacks/')).length, 2);
  const mutations = f.calls.filter(c => /\/messages\/[^/]+\/(reaction|delete)$/.test(c.path) && !c.body.connection_ref);
  for (const effect of f.effectCalls) {
    const pair = mutations.filter(c => digest(c.key) === effect.operation_key_sha256);
    assert.equal(pair.length, 2); assert.notEqual(pair[0].slot, pair[1].slot);
    assert.deepEqual(pair[0].body, pair[1].body);
  }
  for (const region of f.lease.regions) {
    const original = f.initial.records.get(region.relay.channelId), saved = f.stored.get(region.relay.channelId);
    assert.deepEqual(saved.definition.sources, original.definition.sources); assert.equal(saved.version, original.version + 2);
  }
  assert.equal(f.observations.filter(o => o.label === 'eg_am_evidence').length, 2);
  assert.equal(f.observations.filter(o => o.label === 'lark_relay_finality').length, 1);
});

test('an uncertain relay mutation stops the journey before replay or another effect', async t => {
  const f = await orchestration(t, 'unknown_effect');
  await assert.rejects(runLarkRelayResources(f.context, f.lease, f.initial, f.nativeTurns), /lark_relay_public_effect_confirmed/);
  assert.equal(f.effectCalls.length, 1); assert.equal(f.events.size, 1);
  assert.equal(f.calls.filter(c => /\/messages\/[^/]+\/(reaction|delete)$/.test(c.path)).length, 1);
  assert.equal(f.calls.filter(c => c.method === 'PUT').length, 0);
  assert.equal(f.observations.some(o => o.label === 'lark_relay_finality'), false);
});

test('late owned work prevents relay completion even when each observed effect was confirmed', async t => {
  const f = await orchestration(t, 'late_effect');
  await assert.rejects(runLarkRelayResources(f.context, f.lease, f.initial, f.nativeTurns), /lark_relay_all_owned_work_reconciled/);
  assert.equal(f.replies.size, 2);
  assert.equal(f.observations.some(o => o.label === 'lark_relay_finality'), false);
});

test('expected_result rechecks finality after step_2 instead of trusting its earlier successful effects', async t => {
  const f = await orchestration(t);
  const journey = await runLarkRelayResources(f.context, f.lease, f.initial, f.nativeTurns);
  f.effects.push({platformRequestId: randomUUID()});
  await assert.rejects(verifyLarkRelayResult(f.context, f.lease, f.initial, journey), /lark_relay_all_owned_work_reconciled/);
  assert.equal(f.observations.some(o => o.label === 'lark_relay_finality'), false);
  await assert.rejects(verifyLarkRelayResult(f.context, f.lease, f.initial, {resourceEffects: 16, status: 'PASS'}),
    error => error instanceof Blocked && error.message === 'lark_relay_completed_journeys_required');
});
