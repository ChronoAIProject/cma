import test from 'node:test';
import assert from 'node:assert/strict';
import {createServer} from 'node:http';
import {once} from 'node:events';
import {createDecipheriv, createHash, randomBytes, randomUUID} from 'node:crypto';
import {Blocked} from '../../runner.mjs';
import {digest} from './helpers.mjs';
import {createNativeLarkWebhook} from './native-lark-webhook.mjs';
import {nativeLarkEventId, validateLarkNativeFixture, assertLarkNativeCustody, assertLarkNativeIngress,
  assertLarkNativeAssociation, assertLarkNativeContext, assertLarkNativeDelivery, assertLarkNativeActivity,
  runLarkNativeIntake} from './lark-native-journey.mjs';

const ctx = {assert: (value, code) => assert.ok(value, code)};
const source = 'a'.repeat(40), image = 'fixture@sha256:' + 'b'.repeat(64);
const stamp = new Date().toISOString();
const origin = region => region === 'lark' ? 'https://open.larksuite.com' : 'https://open.feishu.cn';
const credentials = () => Object.fromEntries(['app_secret', 'encrypt_key', 'verification_token']
  .map(k => [k, randomBytes(32).toString('hex')]));

function fixture() {
  const keys = {lark: credentials(), feishu: credentials()};
  const lease = {local: true, synthetic: true, ownerSubject: 'owner', source, image,
    request: async () => {}, counters: async () => {}, close: async () => {},
    regions: ['lark', 'feishu'].map(region => ({region,
      native: {recordId: region + '-native-record', channelId: region + '-native-channel',
        senderId: 'ou_' + region + '_sender', chatId: 'oc_' + region + '_chat', sourceRevision: digest(region),
        identity: {region, app_id: 'cli_' + region + '_native', tenant_key: region + '_tenant', bot_open_id: 'ou_' + region + '_native'},
        credentialDigests: Object.fromEntries(Object.entries(keys[region]).map(([k, v]) => [k, digest(v)]))},
      relay: {recordId: region + '-relay-record', channelId: region + '-relay-channel', senderId: 'ou_' + region + '_relay_sender',
        chatId: 'oc_' + region + '_relay_chat', issuer: 'issuer', keyId: 'key', routeId: region + '-route', botId: 'ou_' + region + '_relay',
        binding: {user_service_id: region + '-selected', catalog_service_id: region + '-catalog', app_id: 'cli_' + region + '_relay'}},
      connections: ['selected', 'unselected'].map(s => ({user_service_id: region + '-' + s, catalog_service_id: region + '-catalog',
        platform: region, source: 'personal', name: 'Synthetic ' + s})),
    })),
    nativeRuntime: {transport: {expectedSource: source, expectedImage: image}, credentials: keys,
      binding: {schema: 'multi-eg-native-lark-local-binding-v1', source, image, artifact_sha256: digest('binding'),
        regions: ['lark', 'feishu'].map(region => ({region, official_origin: origin(region), local_origin: 'http://127.0.0.1:12345'}))}},
    ...Object.fromEntries(['nativeCustody', 'nativeCallbackCapture', 'prepareNativeAnswer', 'waitForTurn', 'nativeEventCapture',
      'providerContext', 'waitForDelivery', 'captureEffects', 'drain'].map(method => [method, async () => {}])),
  };
  return lease;
}
function input(region, replica = 'a') {
  return {replica, eventId: randomUUID(), eventType: 'im.message.receive_v1', completionText: 'Synthetic native answer',
    event: {sender: {sender_type: 'user', sender_id: {open_id: region.native.senderId}, tenant_key: region.native.identity.tenant_key},
      message: {message_id: 'om_original', chat_id: region.native.chatId, chat_type: 'p2p', message_type: 'text',
        create_time: String(Date.now()), content: JSON.stringify({text: 'Synthetic native input'})}}};
}
function event(region) {
  return {eventId: nativeLarkEventId(region.native.identity, 'om_original'), eventRef: 'sealed-synthetic-reference',
    consumerId: 'default', agentId: 'agent', conversationId: 'conversation', responseId: 'response',
    destination: region.native.chatId, expectedSender: region.native.identity.bot_open_id,
    expectedContentDigest: digest('Synthetic native answer')};
}
function platform(region, method, path) {
  return {request_id: randomUUID(), artifact_sha256: digest(path), method, path, status: 200,
    origin: origin(region.region), transport_origin: 'http://127.0.0.1:12345', source, image,
    identity: structuredClone(region.native.identity)};
}
function custody(region) {
  const token = digest(randomBytes(32));
  return {schema: 'multi-eg-native-lark-custody-v1', record_id: region.native.recordId, artifact_sha256: digest('custody'), calls: [
    {...platform(region, 'POST', '/open-apis/auth/v3/tenant_access_token/internal'),
      request: {app_id: region.native.identity.app_id, app_secret_sha256: region.native.credentialDigests.app_secret},
      response: {code: 0, tenant_access_token_sha256: token, expire: 7200}},
    {...platform(region, 'GET', '/open-apis/bot/v3/info'), credential_sha256: token,
      response: {code: 0, bot: {open_id: region.native.identity.bot_open_id}}},
  ]};
}
function association(region) {
  const ev = event(region);
  return {...ev, schema: 'multi-eg-native-event-v1', seal_verified: true, request_id: 'am-actual-request',
    artifact_sha256: digest('event'), event_ref_sha256: digest(ev.eventRef), source: {type: 'native_lark', ...region.native.identity},
    actor_id: region.native.senderId, text_sha256: digest('Synthetic native input'),
    reply: {type: 'native_lark', mentioned: false, identity: structuredClone(region.native.identity),
      chat_id: region.native.chatId, message_id: 'om_original', thread_id: null, revision: region.native.sourceRevision}};
}
function context(region) {
  return {activity_verified: true, event_id: event(region).eventId, channel_id: region.native.channelId,
    conversation_id: 'conversation', consumer: {id: 'default'}, source: {type: 'native_lark', ...region.native.identity},
    activity: {actor: {id: region.native.senderId, kind: 'human'}, conversation: {id: region.native.chatId, kind: 'private'},
      kind: {type: 'message', text: 'Synthetic native input', attachments: [], mentions_bot: false}},
    capabilities: {buttons: 'numbered_text', cards: 'text', max_text_chars: 6000}};
}
function delivery(region) {
  const ev = event(region), auth = custody(region);
  return {eventId: ev.eventId, responseId: ev.responseId, destination: ev.destination, sender: ev.expectedSender,
    contentDigest: ev.expectedContentDigest, status: 'delivered', count: 1,
    platformReceipt: {...platform(region, 'POST', '/open-apis/im/v1/messages?receive_id_type=chat_id'),
      schema: 'multi-eg-native-lark-delivery-v1',
      custody: auth, credential_sha256: auth.calls[0].response.tenant_access_token_sha256,
      request: {receive_id: region.native.chatId, msg_type: 'text', content_sha256: digest(JSON.stringify({text: 'Synthetic native answer'}))},
      response: {code: 0, data: {message_id: 'om_answer', chat_id: region.native.chatId}}}};
}

test('native event identity matches Rust declaration-order bytes and changes with exact app custody', () => {
  const identity = {bot_open_id: 'ou_bot', tenant_key: 'tenant', app_id: 'cli_app', region: 'lark'};
  const bytes = '["message",{"region":"lark","app_id":"cli_app","tenant_key":"tenant","bot_open_id":"ou_bot"},"om_original"]';
  const expected = 'native-lark:' + createHash('sha256').update(bytes).digest('hex');
  assert.equal(nativeLarkEventId(identity, 'om_original'), expected);
  for (const field of ['region', 'app_id', 'tenant_key', 'bot_open_id'])
    assert.notEqual(nativeLarkEventId({...identity, [field]: 'different'}, 'om_original'), expected);
  assert.notEqual(nativeLarkEventId(identity, 'om_other'), expected);
});

test('native intake blocks unsupported regional transports or missing independent captures before callbacks', async () => {
  validateLarkNativeFixture(fixture());
  for (const edit of [f => delete f.nativeCallbackCapture, f => delete f.nativeRuntime.binding,
    f => f.nativeRuntime.binding.regions[0].local_origin = 'https://open.larksuite.com',
    f => f.nativeRuntime.binding.regions[0].official_origin = 'https://open.feishu.cn',
    f => f.nativeRuntime.transport.expectedSource = 'c'.repeat(40),
    f => f.regions[0].native.sourceRevision = 'sha256:' + digest('revision'),
    f => f.regions[0].native.senderId = f.regions[0].native.identity.bot_open_id,
    f => f.nativeRuntime.credentials.lark.encrypt_key = randomBytes(32).toString('hex')]) {
    const f = fixture(); edit(f); let calls = 0; f.prepareNativeAnswer = async () => {calls++;};
    await assert.rejects(runLarkNativeIntake(ctx, f), error => error instanceof Blocked);
    assert.equal(calls, 0);
  }
});

test('native custody requires actual regional app-token and bot-info responses joined by token digest', () => {
  const f = fixture(), r = f.regions[0]; assertLarkNativeCustody(ctx, f, r, custody(r));
  for (const edit of [c => c.calls[0].origin = 'https://open.feishu.cn', c => c.calls[1].response.bot.open_id = r.relay.botId,
    c => c.calls[1].credential_sha256 = digest('other'), c => c.calls[0].request.app_id = r.relay.binding.app_id,
    c => c.calls[0].request.app_secret_sha256 = digest('other'), c => c.calls[0].response.code = 1,
    c => c.calls[0].response.expire = 0, c => c.calls[1].artifact_sha256 = '', c => c.record_id = r.relay.recordId]) {
    const c = custody(r); edit(c); assert.throws(() => assertLarkNativeCustody(ctx, f, r, c), /lark_native_/);
  }
});

test('sealed native event references require the original app, message and a captured AM request', () => {
  const f = fixture(), r = f.regions[0], turn = {amRequests: [{request_id: 'am-actual-request'}]};
  assertLarkNativeAssociation(ctx, r, input(r), event(r), turn, association(r));
  for (const edit of [c => c.seal_verified = false, c => c.request_id = 'eg-request-not-am',
    c => c.reply.type = 'nyxid_relay', c => c.reply.revision = 'sha256:' + r.native.sourceRevision,
    c => c.reply.identity.app_id = r.relay.binding.app_id, c => c.reply.message_id = 'om_foreign',
    c => c.actor_id = r.relay.senderId, c => c.text_sha256 = digest('other'), c => c.consumerId = 'other',
    c => c.event_ref_sha256 = digest('unverified-reference')]) {
    const c = association(r); edit(c);
    assert.throws(() => assertLarkNativeAssociation(ctx, r, input(r), event(r), turn, c), /lark_native_/);
  }
});

test('public native context keeps original authority and does not offer disabled interactive forms or cards', () => {
  const r = fixture().regions[1]; assertLarkNativeContext(ctx, r, input(r), event(r), context(r));
  for (const edit of [c => c.activity_verified = false, c => c.source.type = 'nyxid_relay',
    c => c.source.region = 'lark', c => c.consumer.id = 'other', c => c.activity.kind.text = 'other',
    c => c.activity.conversation.id = r.relay.chatId, c => c.capabilities.forms = true,
    c => c.capabilities.buttons = 'native_interactive', c => c.capabilities.cards = 'native_interactive']) {
    const c = context(r); edit(c); assert.throws(() => assertLarkNativeContext(ctx, r, input(r), event(r), c), /lark_native_/);
  }
});

test('native delivery proves the exact new platform message and double-encoded content', () => {
  const f = fixture(), r = f.regions[1]; assertLarkNativeDelivery(ctx, f, r, input(r), event(r), delivery(r));
  for (const edit of [d => d.platformReceipt.origin = 'https://open.larksuite.com', d => d.status = 'accepted',
    d => d.platformReceipt.request.content_sha256 = digest('Synthetic native answer'),
    d => d.platformReceipt.request.receive_id = r.relay.chatId, d => d.platformReceipt.response.data.message_id = 'om_original',
    d => d.platformReceipt.response.data.thread_id = 'omt_foreign', d => d.replyToMessageId = 'om_original',
    d => d.platformReceipt.path = '/api/v1/relay/reply', d => d.platformReceipt.artifact_sha256 = '', d => d.count = 2,
    d => d.platformReceipt.credential_sha256 = digest('wrong-token'), d => delete d.platformReceipt.custody,
    d => d.platformReceipt.custody.calls[1].response.bot.open_id = r.relay.botId]) {
    const d = delivery(r); edit(d); assert.throws(() => assertLarkNativeDelivery(ctx, f, r, input(r), event(r), d), /lark_native_|reply_/);
  }
});

test('native Activity requires complete original-source turns with one confirmed answer', () => {
  const r = fixture().regions[0], ev = event(r), turns = [{region: r.region, event: ev, input: input(r)}];
  const page = () => ({cursor: 'cursor', retention_days: 7, limit: 50, has_more: false, items: [{event_id: ev.eventId,
    source: {type: 'native_lark', ...r.native.identity}, actor: {id: r.native.senderId, kind: 'human'},
    conversation: {id: r.native.chatId, kind: 'private'}, received_at: stamp, occurred_at: stamp, consumers: [{consumer_id: 'default',
      agent_id: ev.agentId, response_id: ev.responseId, conversation_id: ev.conversationId, state: 'completed', dispatched_at: stamp,
      accepted_at: stamp, produced_at: stamp, completed_at: stamp,
      deliveries: [{step: 'answer', outcome: 'confirmed', observed_at: stamp, confirmed_at: stamp}]}]}]});
  assertLarkNativeActivity(ctx, r, page(), turns);
  for (const edit of [p => p.items[0].source.type = 'nyxid_relay', p => p.items[0].consumers[0].state = 'accepted',
    p => p.items[0].consumers[0].deliveries[0].outcome = 'unknown', p => p.items[0].actor.id = r.relay.senderId,
    p => p.items[0].consumers[0].agent_id = 'other', p => p.has_more = true, p => p.items.push({...p.items[0]})]) {
    const p = page(); edit(p); assert.throws(() => assertLarkNativeActivity(ctx, r, p, turns), /lark_native_|activity_/);
  }
});

// Actual encrypted bytes are independently received and decrypted at two local
// HTTP servers. Neither server is the Rust handler; this is an evidence-join test.
async function wireFixture(t) {
  const lease = fixture(), region = lease.regions[0], received = new Map(), members = {};
  for (const slot of ['a', 'b']) {
    const server = createServer(async (req, res) => {
      const chunks = []; for await (const chunk of req) chunks.push(chunk); const bytes = Buffer.concat(chunks);
      const encrypted = Buffer.from(JSON.parse(bytes).encrypt, 'base64'), keys = lease.nativeRuntime.credentials.lark;
      const decrypt = createDecipheriv('aes-256-cbc', createHash('sha256').update(keys.encrypt_key).digest(), encrypted.subarray(0, 16));
      const payload = JSON.parse(Buffer.concat([decrypt.update(encrypted.subarray(16)), decrypt.final()]));
      const challenge = payload.type === 'url_verification', token = challenge ? payload.token : payload.header.token;
      if (challenge) delete payload.token; else delete payload.header.token;
      const requestId = slot + '-' + received.size, signature = req.headers['x-lark-signature'];
      const valid = challenge || signature === createHash('sha256').update(req.headers['x-lark-request-timestamp'])
        .update(req.headers['x-lark-request-nonce']).update(keys.encrypt_key).update(bytes).digest('hex');
      received.set(requestId, {schema: 'multi-eg-native-lark-wire-v1', provenance: 'receiver', request_id: requestId,
        method: req.method, path: req.url, artifact_sha256: digest(Buffer.concat(chunks)), body_sha256: digest(bytes),
        payload, payload_sha256: digest(payload), encrypt_key_sha256: digest(keys.encrypt_key), verification_token_sha256: digest(token),
        signature_sha256: signature ? digest(signature) : null, nonce_sha256: req.headers['x-lark-request-nonce']
          ? digest(req.headers['x-lark-request-nonce']) : null, timestamp: req.headers['x-lark-request-timestamp'] ?? null});
      res.writeHead(valid ? 200 : 401, {'content-type': valid ? 'application/json' : 'application/problem+json', 'x-request-id': requestId});
      res.end(JSON.stringify(valid ? challenge ? {challenge: payload.challenge} : {accepted: true}
        : {type: 'about:blank', title: 'not authenticated', status: 401, code: 'not_authenticated', detail: 'native_lark_authentication_failed'}));
    });
    server.listen(0, '127.0.0.1'); await once(server, 'listening');
    t.after(() => new Promise(resolve => {server.close(resolve); server.closeAllConnections();}));
    members[slot] = {origin: 'http://127.0.0.1:' + server.address().port, instance: 'eg-' + slot, instance_id: 'eg-' + slot,
      pod_uid: 'pod-' + slot, container_id: 'container-' + slot, ready: true, source, image};
  }
  const sender = await createNativeLarkWebhook({scope: 'LOCAL_DISPOSABLE_TWO_AM', synthetic: true, shared_database: true,
    expectedSource: source, expectedImage: image, topology: async () => members, authorize: async () => ({}),
    witness: async ({replica, responseRequestId}) => ({slot: replica, instance: 'eg-' + replica, requestId: responseRequestId})},
  {local: true, synthetic: true, channelId: region.native.channelId, identity: region.native.identity,
    credentials: lease.nativeRuntime.credentials.lark});
  t.after(() => sender.close()); return {lease, region, sender, received};
}

test('native ingress joins both actual EGs to receiver-decrypted events and unsigned challenges', async t => {
  const f = await wireFixture(t);
  for (const replica of ['a', 'b']) {
    const value = input(f.region, replica), response = await f.sender.sendEvent(value);
    assertLarkNativeIngress(ctx, f.lease, f.region, value, response, f.received.get(response.requestId), {status: 200, body: {accepted: true}});
    const challenge = {replica, challenge: randomUUID()}, reply = await f.sender.sendChallenge(challenge);
    assertLarkNativeIngress(ctx, f.lease, f.region, challenge, reply, f.received.get(reply.requestId), {status: 200, body: {challenge: challenge.challenge}});
  }
  assert.equal(f.received.size, 4);
});

test('sender provenance, wrong request joins and generic HTTP success cannot establish native admission', async t => {
  const f = await wireFixture(t), value = input(f.region), response = await f.sender.sendEvent(value);
  for (const edit of [c => c.provenance = 'sender', c => c.request_id = 'another-request',
    c => c.path = '/v1/callbacks/' + f.region.native.channelId, c => c.payload.header.app_id = f.region.relay.binding.app_id,
    c => c.signature_sha256 = digest('other'), c => c.payload_sha256 = digest('other'), c => c.artifact_sha256 = '']) {
    const c = structuredClone(f.received.get(response.requestId)); edit(c);
    assert.throws(() => assertLarkNativeIngress(ctx, f.lease, f.region, value, response, c, {status: 200, body: {accepted: true}}), /lark_native_/);
  }
  const invalid = {...input(f.region, 'b'), variant: 'invalid_signature'};
  const refusal = await f.sender.sendEvent({...invalid, allowed: [401]});
  assertLarkNativeIngress(ctx, f.lease, f.region, invalid, refusal, f.received.get(refusal.requestId), {status: 401});
  assert.throws(() => assertLarkNativeIngress(ctx, f.lease, f.region, value, {...response, body: {status: 'ok'}},
    f.received.get(response.requestId), {status: 200, body: {accepted: true}}), /exact_admission_response/);
});
