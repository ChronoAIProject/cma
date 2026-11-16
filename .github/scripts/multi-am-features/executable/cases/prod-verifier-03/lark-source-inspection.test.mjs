import test from 'node:test';
import assert from 'node:assert/strict';
import {createServer} from 'node:http';
import {once} from 'node:events';
import {Blocked} from '../../runner.mjs';
import {createEgTransport, setups} from './setup.mjs';
import {validateLarkInspectionFixture, assertLarkCapabilities, assertLarkSource,
  assertLarkConnections, readLarkSources} from './lark-source-inspection.mjs';

const ctx = {assert: (value, code) => assert.ok(value, code), am: 'a', observe: async (_label, read) => read()};
const source = 'a'.repeat(40), image = 'fixture@sha256:' + 'b'.repeat(64);
const caps = () => ({relay_authoring: {native_lark_writes_enabled: true, native_lark_intake_enabled: true,
  lark_resource_writes_enabled: true, lark_message_effects_writes_enabled: true, multi_consumer_writes_enabled: true,
  native_lark_controls_enabled: false, native_lark_forms_enabled: false,
  platforms: ['lark', 'feishu'].map(id => ({id, verification_modes: [{id: 'native_event', writes_enabled: true}]}))}});
function fixture() {
  return {local: true, synthetic: true, ownerSubject: 'fixture-owner', source, image,
    replicas: ['a', 'b'].map(slot => ({slot, instance: 'eg-' + slot, image, ready: true})),
    request: async () => {}, counters: async () => {}, close: async () => {},
    regions: ['lark', 'feishu'].map(region => ({region,
      native: {recordId: region + '-native-record', channelId: region + '-native-channel',
        senderId: 'ou_' + region + '_native_sender', chatId: 'oc_' + region + '_native_chat',
        identity: {region, app_id: 'cli_' + region + '_native_app', tenant_key: region + '_tenant', bot_open_id: 'ou_' + region + '_native_bot'}},
      relay: {recordId: region + '-relay-record', channelId: region + '-relay-channel',
        senderId: 'ou_' + region + '_relay_sender', chatId: 'oc_' + region + '_relay_chat', issuer: 'synthetic-relay',
        keyId: region + '-key', routeId: region + '-route', botId: 'ou_' + region + '_relay_bot',
        binding: {user_service_id: region + '-selected', catalog_service_id: region + '-catalog', app_id: 'cli_' + region + '_relay_app'}},
      connections: ['selected', 'unselected'].map(selection => ({user_service_id: region + '-' + selection,
        catalog_service_id: region + '-catalog', name: 'Synthetic ' + selection, platform: region, source: 'personal'})),
    }))};
}
function record(lease, region, kind) {
  const seed = region[kind], admission = {type: 'scoped', senders: {type: 'allowlist', ids: [seed.senderId]},
    chats: {type: 'allowlist', ids: [seed.chatId]}, groups: 'mention'};
  const source = kind === 'native' ? {type: 'native_lark', separate_app: true, identity: structuredClone(seed.identity), admission}
    : {type: 'nyxid_relay', platform: region.region, issuer: seed.issuer, key_id: seed.keyId, route_ids: [seed.routeId],
      bot_id: seed.botId, lark_resources: structuredClone(seed.binding),
      lark_message_effects: {connection: structuredClone(seed.binding), delete: true, reaction: true}, admission};
  return {record_id: seed.recordId, channel_id: seed.channelId, version: 1, provider: {slug: 'cma_codex'},
    endpoints: {native_lark_webhook_url: 'https://synthetic.invalid/v1/native-lark/' + seed.channelId},
    definition: {name: region.region + ' ' + kind + ' fixture', owner: {subject: lease.ownerSubject},
      creation: {ready: true}, sources: [source]}};
}

test('Lark inspection requires two regions with separate native apps and relay connections', () => {
  validateLarkInspectionFixture(fixture());
  for (const edit of [f => f.local = false, f => f.source = '', f => f.regions.pop(), f => delete f.counters,
    f => f.regions[1].region = 'lark', f => f.regions[0].relay.channelId = f.regions[0].native.channelId,
    f => f.regions[0].relay.binding.app_id = f.regions[0].native.identity.app_id,
    f => f.regions[0].relay.botId = f.regions[0].native.identity.bot_open_id,
    f => f.regions[0].connections[1] = {...f.regions[0].connections[0]},
    f => f.regions[0].connections[0].catalog_service_id = 'other-catalog',
    f => f.regions[0].connections[1].platform = 'feishu']) {
    const f = fixture(); edit(f);
    assert.throws(() => validateLarkInspectionFixture(f), error => error instanceof Blocked);
  }
});

test('native source inspection refuses relay bindings, wrong app and widened admission', () => {
  const f = fixture(), region = f.regions[0];
  assertLarkSource(ctx, f, region, 'native', record(f, region, 'native'));
  for (const edit of [r => r.definition.sources[0].lark_resources = region.relay.binding,
    r => r.definition.sources[0].lark_message_effects = {connection: region.relay.binding},
    r => r.definition.sources[0].identity.app_id = region.relay.binding.app_id,
    r => r.definition.sources[0].separate_app = false, r => r.definition.sources[0].route_ids = ['relay-route'],
    r => r.definition.sources[0].admission.senders = {type: 'any'},
    r => r.definition.sources[0].admission.chats.ids.push('oc_foreign'),
    r => r.definition.sources[0].admission.bot_messages = 'allow',
    r => r.definition.sources.push({...r.definition.sources[0]}), r => r.definition.owner.subject = 'other-owner',
    r => r.channel_id = region.relay.channelId]) {
    const r = record(f, region, 'native'); edit(r);
    assert.throws(() => assertLarkSource(ctx, f, region, 'native', r), /lark_/);
  }
});

test('relay source inspection requires the selected binding and consent on the exact bot and route', () => {
  const f = fixture(), region = f.regions[1];
  assertLarkSource(ctx, f, region, 'relay', record(f, region, 'relay'));
  for (const edit of [r => r.definition.sources[0].lark_resources.user_service_id = 'unselected',
    r => r.definition.sources[0].lark_message_effects.connection.catalog_service_id = 'different',
    r => r.definition.sources[0].lark_message_effects.delete = false,
    r => r.definition.sources[0].lark_message_effects.reaction = false,
    r => r.definition.sources[0].identity = region.native.identity,
    r => r.definition.sources[0].trust_normalized = true,
    r => r.definition.sources[0].platform = 'lark', r => r.definition.sources[0].route_ids.push('other-route'),
    r => r.definition.sources[0].bot_id = region.native.identity.bot_open_id]) {
    const r = record(f, region, 'relay'); edit(r);
    assert.throws(() => assertLarkSource(ctx, f, region, 'relay', r), /lark_/);
  }
});

test('capability evidence cannot substitute an enabled controls fixture or another relay mode', () => {
  assertLarkCapabilities(ctx, caps());
  for (const key of ['native_lark_writes_enabled', 'native_lark_intake_enabled', 'lark_resource_writes_enabled',
    'lark_message_effects_writes_enabled', 'multi_consumer_writes_enabled', 'native_lark_controls_enabled', 'native_lark_forms_enabled']) {
    const value = caps(); value.relay_authoring[key] = !value.relay_authoring[key];
    assert.throws(() => assertLarkCapabilities(ctx, value), error => error instanceof Blocked);
  }
  const value = caps(); value.relay_authoring.platforms[0].verification_modes[0].id = 'trusted_normalized';
  assert.throws(() => assertLarkCapabilities(ctx, value), /actual_regional_relay/);
});

test('connection discovery checks exact inventory without treating selection as access authority', () => {
  const region = fixture().regions[0];
  assertLarkConnections(ctx, region, {items: structuredClone(region.connections).reverse()});
  for (const edit of [v => v.items.pop(), v => v.items.push({...v.items[0]}),
    v => v.items[0].platform = 'feishu', v => v.items[0].source = 'foreign',
    v => v.items[0].catalog_service_id = 'other-catalog', v => v.items[0].authorized = true]) {
    const value = {items: structuredClone(region.connections)}; edit(value);
    assert.throws(() => assertLarkConnections(ctx, region, value), /lark_discovery/);
  }
});

// These two loopback servers exercise actual GET routing and serving receipts.
// They are protocol fixtures, not running EG binaries or a compiled UI.
async function httpFixture(t, edit = () => {}) {
  const lease = fixture(), members = {}, calls = [], observations = [];
  for (const slot of ['a', 'b']) {
    const server = createServer((req, res) => {
      calls.push({slot, method: req.method, path: req.url});
      const url = new URL(req.url, 'http://127.0.0.1'); let body;
      if (url.pathname === '/v1/channel-capabilities') body = caps();
      else if (url.pathname === '/v1/relay-resource-connections') {
        const region = lease.regions.find(r => r.region === url.searchParams.get('platform'));
        body = {items: structuredClone(region.connections)};
      } else {
        for (const region of lease.regions) for (const kind of ['native', 'relay'])
          if (url.pathname === '/v1/channels/' + region[kind].channelId) body = record(lease, region, kind);
      }
      edit(slot, url.pathname, body);
      res.writeHead(200, {'content-type': 'application/json', 'x-request-id': slot + '-' + calls.length});
      res.end(JSON.stringify(body));
    });
    server.listen(0, '127.0.0.1'); await once(server, 'listening');
    t.after(() => new Promise(resolve => {server.close(resolve); server.closeAllConnections();}));
    members[slot] = {origin: 'http://127.0.0.1:' + server.address().port, instance: 'eg-' + slot,
      instance_id: 'eg-' + slot, pod_uid: 'pod-' + slot, container_id: 'container-' + slot, ready: true, source, image};
  }
  const transport = await createEgTransport({scope: 'LOCAL_DISPOSABLE_TWO_AM', synthetic: true, shared_database: true,
    expectedSource: source, expectedImage: image, topology: async () => members, authorize: async () => ({}),
    witness: async ({replica, responseRequestId}) => ({slot: replica, instance: 'eg-' + replica, requestId: responseRequestId})});
  Object.assign(lease, transport);
  return {lease, calls, context: {...ctx, observe: async (label, read) => {const value = await read(); observations.push({label, value}); return value;}}, observations};
}

test('both actual EG transports inspect both regions and preserve full GET receipts', async t => {
  const {lease, calls, context, observations} = await httpFixture(t);
  const first = await readLarkSources(context, lease);
  await readLarkSources(context, lease, first);
  assert.equal(first.records.size, 4); assert.equal(calls.length, 28);
  assert.ok(calls.every(c => c.method === 'GET'));
  for (const slot of ['a', 'b']) for (const region of ['lark', 'feishu']) {
    for (const kind of ['native', 'relay']) assert.equal(calls.filter(c => c.slot === slot
      && c.path === '/v1/channels/' + region + '-' + kind + '-channel').length, 2);
    assert.equal(calls.filter(c => c.slot === slot && c.path === '/v1/relay-resource-connections?platform=' + region).length, 2);
  }
  const receipts = observations.filter(o => o.value.schema === 'multi-eg-request-v1').map(o => o.value.receipt);
  assert.equal(receipts.length, 28); assert.equal(new Set(receipts.map(r => r.request_id)).size, 28);
  assert.ok(receipts.every(r => r.serving.source === source && r.serving_after.source === source));
  assert.equal(observations.some(o => o.value.schema === 'multi-am-eg-evidence-v1'), false);
});

test('replica divergence and a later source edit cannot pass the read-only inspection', async t => {
  const divergent = await httpFixture(t, (slot, _path, body) => {
    if (slot === 'b' && body?.definition) body.version = 2;
  });
  await assert.rejects(readLarkSources(divergent.context, divergent.lease), /same_saved_source_on_both/);
  let changed = false;
  const later = await httpFixture(t, (_slot, _path, body) => {if (changed && body?.definition) body.version = 2;});
  const before = await readLarkSources(later.context, later.lease); changed = true;
  await assert.rejects(readLarkSources(later.context, later.lease, before), /saved_source_unchanged/);
});

test('HTTP success with requested labels or a different serving build does not establish inspection', async t => {
  for (const edit of [r => delete r.eg_receipt, r => r.eg_receipt.serving.source = 'c'.repeat(40),
    r => r.eg_receipt.replica = 'b']) {
    const {lease, context} = await httpFixture(t), request = lease.request;
    lease.request = async input => {const response = await request(input); edit(response); return response;};
    await assert.rejects(readLarkSources(context, lease), /actual_eg_source_image_receipt/);
  }
});

test('named inspection setup registers cleanup before refusing conflated native and relay custody', async () => {
  const lease = fixture(), deferred = []; let closed = 0;
  lease.close = async () => {closed++; throw Error('retained_cleanup_failure');};
  lease.regions[0].relay.botId = lease.regions[0].native.identity.bot_open_id;
  await assert.rejects(setups.native_lark_resources({row: {case_id: 'C275:EG-043'},
    defer: (_label, fn) => deferred.push(fn)}, {eg: {acquire: async () => lease}}), /custody_must_be_distinct/);
  assert.equal(deferred.length, 1);
  await assert.rejects(deferred[0](), /retained_cleanup_failure/);
  await assert.rejects(deferred[0](), /retained_cleanup_failure/); assert.equal(closed, 1);
});
