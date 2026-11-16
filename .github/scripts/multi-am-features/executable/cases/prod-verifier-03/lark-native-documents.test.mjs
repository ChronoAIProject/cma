import test from 'node:test';
import assert from 'node:assert/strict';
import {createServer} from 'node:http';
import {once} from 'node:events';
import {randomBytes, randomUUID} from 'node:crypto';
import {Blocked} from '../../runner.mjs';
import {digest} from './helpers.mjs';
import {createEgTransport} from './setup.mjs';
import {nativeLarkEventId, assertLarkNativeAssociation, assertLarkNativeContext} from './lark-native-journey.mjs';
import {validateLarkDocumentFixture, nativeLarkDocument, larkDocumentAttachment,
  assertLarkDocumentView, readLarkDocument, assertLarkResourceCalls, runLarkNativeDocuments} from './lark-native-documents.mjs';

const source = 'a'.repeat(40), image = 'fixture@sha256:' + 'b'.repeat(64);
const ctx = {am: 'a', assert: (value, code) => assert.ok(value, code), observe: async (_, read) => read()};
function fixture() {
  const credentials = Object.fromEntries(['lark', 'feishu'].map(region => [region,
    Object.fromEntries(['app_secret', 'encrypt_key', 'verification_token'].map(k => [k, randomBytes(32).toString('hex')]))]));
  const regions = ['lark', 'feishu'].map(region => ({region,
    native: {recordId: region + '-native-record', channelId: region + '-native-channel', senderId: 'ou_' + region + '_sender',
      chatId: 'oc_' + region + '_chat', sourceRevision: digest(region), identity: {region, app_id: 'cli_' + region + '_native',
        tenant_key: region + '_tenant', bot_open_id: 'ou_' + region + '_native'},
      credentialDigests: Object.fromEntries(Object.entries(credentials[region]).map(([k, v]) => [k, digest(v)]))},
    relay: {recordId: region + '-relay-record', channelId: region + '-relay-channel', senderId: 'ou_' + region + '_relay_sender',
      chatId: 'oc_' + region + '_relay_chat', issuer: 'issuer', keyId: 'key', routeId: region + '-route', botId: 'ou_' + region + '_relay',
      binding: {user_service_id: region + '-selected', catalog_service_id: region + '-catalog', app_id: 'cli_' + region + '_relay'}},
    connections: ['selected', 'unselected'].map(s => ({user_service_id: region + '-' + s, catalog_service_id: region + '-catalog',
      platform: region, source: 'personal', name: 'Synthetic ' + s}))}));
  const binding = {source, image, artifact_sha256: digest('binding'), regions: ['lark', 'feishu'].map(region => ({region,
    official_origin: region === 'lark' ? 'https://open.larksuite.com' : 'https://open.feishu.cn', local_origin: 'http://127.0.0.1:12345'}))};
  return {local: true, synthetic: true, ownerSubject: 'owner', source, image, regions,
    nativeRuntime: {transport: {expectedSource: source, expectedImage: image}, credentials,
      binding: {...binding, schema: 'multi-eg-native-lark-local-binding-v1'}},
    nativeMediaBinding: {...structuredClone(binding), schema: 'multi-eg-native-lark-media-binding-v1'},
    ...Object.fromEntries(['request', 'counters', 'close', 'nativeCustody', 'nativeCallbackCapture', 'prepareNativeAnswer',
      'waitForTurn', 'nativeEventCapture', 'providerContext', 'waitForDelivery', 'captureEffects', 'drain',
      'prepareNativeDocument', 'captureNativeResources'].map(k => [k, async () => {}]))};
}
function document(region, replica = 'a') {
  const input = nativeLarkDocument(region, replica);
  return {region: region.region, input, event: {eventId: nativeLarkEventId(region.native.identity, input.event.message.message_id),
    eventRef: 'sealed-' + region.region, agentId: 'agent-' + region.region, responseId: 'response-' + region.region,
    conversationId: 'conversation-' + region.region, consumerId: 'default'}};
}
function view(input, page = 1) {
  return {index: 0, kind: 'file', filename: input.document.filename, detected_type: 'text/plain',
    status: page === 1 ? 'ready' : 'unavailable', ...(page === 1 ? {} : {reason: 'not_found'}),
    text: {source: 'document', content: page === 1 ? input.document.content : '', page, pages: 1, truncated: false}};
}
function resource(lease, doc) {
  const region = lease.regions.find(r => r.region === doc.region), binding = lease.nativeMediaBinding.regions.find(r => r.region === doc.region);
  const base = {method: 'GET', status: 200, origin: binding.official_origin, transport_origin: binding.local_origin,
    source, image, identity: structuredClone(region.native.identity)};
  const call = (method, path) => ({...base, method, path, request_id: randomUUID(), artifact_sha256: digest(path)});
  const token = digest(randomBytes(32));
  const custody = {schema: 'multi-eg-native-lark-custody-v1', record_id: region.native.recordId,
    artifact_sha256: digest('custody'), calls: [
      {...call('POST', '/open-apis/auth/v3/tenant_access_token/internal'),
        request: {app_id: region.native.identity.app_id, app_secret_sha256: region.native.credentialDigests.app_secret},
        response: {code: 0, tenant_access_token_sha256: token, expire: 7200}},
      {...call('GET', '/open-apis/bot/v3/info'), credential_sha256: token,
        response: {code: 0, bot: {open_id: region.native.identity.bot_open_id}}}]};
  return {...call('GET', '/open-apis/im/v1/messages/' + doc.input.event.message.message_id + '/resources/' + doc.input.document.fileKey + '?type=file'),
    schema: 'multi-eg-native-lark-resource-v1', content_type: 'text/plain', bytes: Buffer.byteLength(doc.input.document.content),
    body_sha256: digest(doc.input.document.content), custody, credential_sha256: token};
}

test('native documents use fresh authenticated file locators with exact source revision', () => {
  const r = fixture().regions[0], first = nativeLarkDocument(r, 'a'), next = nativeLarkDocument(r, 'b');
  assert.notEqual(first.document.fileKey, next.document.fileKey);
  assert.notEqual(first.event.message.message_id, next.event.message.message_id);
  assert.notEqual(first.document.content, next.document.content);
  assert.deepEqual(JSON.parse(first.event.message.content), {file_key: first.document.fileKey, file_name: first.document.filename});
  assert.deepEqual(larkDocumentAttachment(r, first), {reference: 'native-lark:' + first.event.message.message_id + ':' + first.document.fileKey,
    kind: 'file', filename: first.document.filename, source: {type: 'native_lark', message_id: first.event.message.message_id,
      resource_key: first.document.fileKey, resource_kind: 'file', source_revision: r.native.sourceRevision}});
  assert.ok(Buffer.byteLength(first.document.content) > first.document.content.length);
});

test('native client transport alone cannot authorize the separate pinned media path', async () => {
  validateLarkDocumentFixture(fixture());
  for (const edit of [f => delete f.nativeMediaBinding, f => f.nativeMediaBinding.source = 'c'.repeat(40),
    f => f.nativeMediaBinding.regions[0].local_origin = 'https://open.larksuite.com',
    f => f.nativeMediaBinding.regions[0].official_origin = 'https://open.feishu.cn',
    f => f.nativeMediaBinding.artifact_sha256 = '', f => delete f.prepareNativeDocument,
    f => delete f.captureNativeResources]) {
    const f = fixture(); edit(f); let calls = 0; f.prepareNativeAnswer = async () => {calls++;};
    await assert.rejects(runLarkNativeDocuments(ctx, f), error => error instanceof Blocked);
    assert.equal(calls, 0);
  }
});

test('sealed native file context carries the exact attachment with empty message text', () => {
  const r = fixture().regions[0], d = document(r), attachment = larkDocumentAttachment(r, d.input);
  const capture = {...d.event, schema: 'multi-eg-native-event-v1', seal_verified: true, request_id: 'actual-am',
    artifact_sha256: digest('sealed-event'), event_ref_sha256: digest(d.event.eventRef), source: {type: 'native_lark', ...r.native.identity},
    actor_id: r.native.senderId, text_sha256: digest(''), attachments: [attachment],
    reply: {type: 'native_lark', mentioned: false, identity: r.native.identity, chat_id: r.native.chatId,
      message_id: d.input.event.message.message_id, thread_id: null, revision: r.native.sourceRevision}};
  assertLarkNativeAssociation(ctx, r, d.input, d.event, {amRequests: [{request_id: 'actual-am'}]}, capture);
  const body = {activity_verified: true, event_id: d.event.eventId, channel_id: r.native.channelId,
    conversation_id: d.event.conversationId, consumer: {id: 'default'}, source: capture.source,
    activity: {actor: {id: r.native.senderId, kind: 'human'}, conversation: {id: r.native.chatId, kind: 'private'},
      kind: {type: 'message', text: '', attachments: [attachment], mentions_bot: false}},
    capabilities: {buttons: 'numbered_text', cards: 'text', max_text_chars: 6000, inbound_media: {read_tool: true}}};
  assertLarkNativeContext(ctx, r, d.input, d.event, body, [attachment]);
  for (const edit of [b => b.activity.kind.attachments[0].source.type = 'lark_resource',
    b => b.activity.kind.attachments[0].source.source_revision = digest('other'),
    b => b.activity.kind.attachments[0].source.resource_key = 'file_other',
    b => b.activity.kind.text = d.input.document.content, b => b.activity.kind.attachments = []]) {
    const bad = structuredClone(body); edit(bad);
    assert.throws(() => assertLarkNativeContext(ctx, r, d.input, d.event, bad, [attachment]), /lark_native_/);
  }
});

test('native document views prove full byte content and exact out-of-range page semantics', () => {
  const d = document(fixture().regions[0]);
  assertLarkDocumentView(ctx, d.input, view(d.input)); assertLarkDocumentView(ctx, d.input, view(d.input, 2), 2);
  for (const edit of [v => v.text.content = 'different', v => v.text.truncated = true, v => v.text.pages = 2,
    v => v.kind = 'image', v => v.status = 'pending', v => v.detected_type = 'application/pdf',
    v => v.text.source = 'transcript', v => v.filename = 'other.txt', v => v.download = {url: 'https://example.invalid'}]) {
    const bad = view(d.input); edit(bad); assert.throws(() => assertLarkDocumentView(ctx, d.input, bad), /lark_document_/);
  }
  const fakePage = view(d.input, 2); fakePage.status = 'ready';
  assert.throws(() => assertLarkDocumentView(ctx, d.input, fakePage, 2), /lark_document_/);
});

test('native resource captures join exact regional bytes and fresh custody without relay authority', () => {
  const f = fixture(), docs = f.regions.map(r => document(r)), calls = docs.map(d => resource(f, d));
  assertLarkResourceCalls(ctx, f, docs, calls);
  for (const edit of [c => c[0].body_sha256 = digest('other'), c => c[0].bytes--,
    c => c[0].path = c[0].path.replace('?type=file', '?type=image'), c => c[0].origin = c[1].origin,
    c => c[0].identity.app_id = f.regions[0].relay.binding.app_id,
    c => c[0].credential_sha256 = digest('other'), c => c[0].user_service_id = f.regions[0].relay.binding.user_service_id,
    c => c[0].custody.calls[1].response.bot.open_id = f.regions[0].relay.botId,
    c => c[0].request_id = c[1].request_id, c => c.push({...c[0], request_id: 'extra', path: '/unscoped'}), c => c.pop()]) {
    const bad = structuredClone(calls); edit(bad);
    assert.throws(() => assertLarkResourceCalls(ctx, f, docs, bad), /lark_document_|lark_native_/);
  }
});

// These are real HTTP reads against protocol doubles, not product execution.
async function readFixture(t) {
  const f = fixture(), region = f.regions[0], doc = document(region), members = {}, requests = [];
  const replies = new Map();
  for (const slot of ['a', 'b']) {
    const server = createServer((req, res) => {
      requests.push({slot, method: req.method, path: req.url});
      const page = Number(new URL(req.url, 'http://localhost').searchParams.get('page'));
      const key = slot + ':' + page, prior = replies.get(key) ?? 0; replies.set(key, prior + 1);
      const body = f.reply ? f.reply({slot, page, prior}) : prior === 0
        ? {index: 0, kind: 'file', filename: doc.input.document.filename, status: 'pending', retry_after_ms: 1} : view(doc.input, page);
      res.writeHead(200, {'content-type': 'application/json', 'x-request-id': slot + '-' + requests.length});
      res.end(JSON.stringify(body));
    });
    server.listen(0, '127.0.0.1'); await once(server, 'listening');
    t.after(() => new Promise(resolve => {server.close(resolve); server.closeAllConnections();}));
    members[slot] = {origin: 'http://127.0.0.1:' + server.address().port, instance: 'eg-' + slot, instance_id: 'eg-' + slot,
      pod_uid: 'pod-' + slot, container_id: 'container-' + slot, ready: true, source, image};
  }
  const transport = await createEgTransport({scope: 'LOCAL_DISPOSABLE_TWO_AM', synthetic: true, shared_database: true,
    expectedSource: source, expectedImage: image, topology: async () => members, authorize: async () => ({}),
    witness: async ({replica, responseRequestId}) => ({slot: replica, instance: 'eg-' + replica, requestId: responseRequestId})});
  f.request = args => transport.request(args);
  f.replicas = ['a', 'b'].map(slot => ({slot, instance: 'eg-' + slot}));
  return {f, region, doc, requests};
}

test('pending native document reads preserve exact GET and actual receipts on both EGs', async t => {
  const {f, region, doc, requests} = await readFixture(t);
  for (const replica of ['a', 'b']) for (const page of [1, 2])
    await readLarkDocument(ctx, f, region, doc.input, doc.event, replica, page);
  assert.equal(requests.length, 8);
  assert.ok(requests.every(r => r.method === 'GET' && r.path.startsWith('/v1/events/' + doc.event.eventRef + '/attachments/0?view=text&page=')));
  assert.equal(new Set(requests.map(r => r.slot)).size, 2);
});

test('a generic success, unavailable content or corrupt read receipt cannot trigger a retry', async t => {
  const {f, region, doc, requests} = await readFixture(t);
  for (const body of [{status: 'ok'}, {...view(doc.input), status: 'unavailable', reason: 'not_found'},
    {index: 0, kind: 'file', filename: doc.input.document.filename, status: 'pending', retry_after_ms: 0}]) {
    f.reply = () => body;
    const before = requests.length;
    await assert.rejects(readLarkDocument(ctx, f, region, doc.input, doc.event, 'a'), /lark_document_/);
    assert.equal(requests.length, before + 1);
  }
  f.reply = () => view(doc.input); const request = f.request;
  f.request = async args => {const result = await request(args); result.eg_receipt.serving.source = 'c'.repeat(40); return result;};
  await assert.rejects(readLarkDocument(ctx, f, region, doc.input, doc.event, 'b'), /actual_read_receipt/);
});

test('native pending document reads abort without another request or event submission', async t => {
  const {f, region, doc, requests} = await readFixture(t), controller = new AbortController();
  f.reply = () => ({index: 0, kind: 'file', filename: doc.input.document.filename, status: 'pending', retry_after_ms: 1000});
  const request = f.request;
  f.request = async args => {const response = await request(args); controller.abort(); return response;};
  await assert.rejects(readLarkDocument({...ctx, signal: controller.signal}, f, region, doc.input, doc.event, 'a'),
    error => error.name === 'AbortError' || /deadline/.test(error.message));
  assert.equal(requests.length, 1); assert.equal(requests[0].method, 'GET');
});
