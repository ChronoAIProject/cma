import test from 'node:test';
import assert from 'node:assert/strict';
import {randomUUID} from 'node:crypto';
import {Blocked} from '../../runner.mjs';
import {digest} from './helpers.mjs';
import {parentEgSetup} from './setup.mjs';
import {editableChannel, validateAdmissionFixture, assertChannelUpdate, guardChannelUpdates,
  assertIngress, assertProviderContext, assertPartitionMapping, assertReplyEffects, assertAdmissionCounters,
  channelAdmissionCase} from './channel-admission.mjs';

const ctx = {assert: (condition, code) => assert.ok(condition, code), observe: async (_label, read) => read()};
function receipt(id = 'request', method = 'POST', status = 202) {
  const serving = {schema: 'c275-eg-serving-v1', scope: 'eg', replica: 'b', origin: 'http://127.0.0.1:4321',
    ready: true, pod_uid: 'pod-b', instance_id: 'eg-b', container_id: 'container-b', source: 'a'.repeat(40),
    image: 'eg@sha256:' + 'b'.repeat(64), observed_at: '2026-11-16T10:00:00.000Z'};
  return {scope: 'eg', replica: 'b', request_id: id, method, status, serving,
    serving_after: {...serving, observed_at: '2026-11-16T10:00:00.003Z'},
    started_at: '2026-11-16T10:00:00.001Z', ended_at: '2026-11-16T10:00:00.002Z',
    attribution: {verdict: 'PASS', basis: 'pod_forward_before_after'}};
}
function fixture() {
  const definition = {schema_version: 1, record_id: 'record', name: 'Synthetic admission',
    creation: {ready: true, fingerprint: 'unchanged'}, owner: {subject: 'synthetic-owner'},
    secrets: {agent_key_hash: 'sha256:' + digest('synthetic-hash-input')}, profile_digest: 'profile-digest',
    created_at: 'retained-time', profile: {metadata: {name: 'Synthetic profile'}},
    provider: {slug: 'cma_codex', binding_id: 'binding', level: 'L2'},
    sources: [{type: 'nyxid_relay', platform: 'telegram', bot_id: 'bot', bot_username: 'syntheticbot',
      issuer: 'local-issuer', jwks_url: 'http://127.0.0.1:4322/jwks', key_id: 'local-key', route_ids: ['local-route'],
      admission: {type: 'all'}}],
    partition: {version: 1, key: 'conversation_and_sender'},
    reply: {auto_final: true, ack_after_ms: 0, sinks: [{type: 'source'}]}};
  const record = {record_id: 'record', channel_id: 'channel', version: 1, definition,
    provider: {slug: 'cma_codex', binding_id: 'binding', level: 'L2'}};
  return {record, recordId: 'record', channelId: 'channel',
    identities: {senders: ['sender-one', 'sender-two'], deniedSender: 'sender-denied', botSender: 'sender-bot',
      chats: ['group-one', 'group-two'], deniedChat: 'group-denied'},
    expected: {replySender: 'bot', ackContentDigest: digest('synthetic-ack')}};
}

test('admission requires an isolated real-verification chat fixture with two distinct senders and chats', () => {
  const f = fixture(); assert.equal(validateAdmissionFixture(f, f.record), f.identities);
  for (const change of [v => {v.identities.senders[1] = v.identities.senders[0];},
    v => {v.identities.deniedSender = v.identities.senders[0];}, v => {v.identities.chats = [];},
    v => {v.record.definition.sources[0].trust_normalized = true;},
    v => {v.record.definition.sources[0].bot_username = '';},
    v => {v.record.definition.sources[0].admission.type = 'allowlist';},
    v => {v.record.definition.routing = {};}, v => {v.record.definition.connection_ref = 'managed';},
    v => {v.record.provider.slug = 'uninspectable-provider';},
    v => {v.record.definition.profile.environment = {session_affinity: 'source_user'};},
    v => {v.record.definition.provider.target = {type: 'existing_agent'};},
    v => {v.record.definition.reply.auto_final = false;}, v => {v.expected.ackContentDigest = '';}]) {
    const bad = fixture(); change(bad);
    assert.throws(() => validateAdmissionFixture(bad, bad.record), error => error instanceof Blocked);
  }
  assert.equal(channelAdmissionCase.id, 'C275:EG-024');
  assert.deepEqual(channelAdmissionCase.steps, ['step_1', 'step_2', 'expected_result']);
  assert.ok(channelAdmissionCase.fixtures.includes('eg_browser'));
  assert.equal(channelAdmissionCase.evidence_scope, undefined, 'AM event proof remains required');
});

function updated(before, draft) {
  const after = structuredClone(before);
  for (const [key, value] of Object.entries(draft)) {
    if (!['provider', 'sources'].includes(key)) after.definition[key] = structuredClone(value);
  }
  after.definition.sources = draft.sources.map((source, index) => ({...structuredClone(source),
    jwks_url: before.definition.sources[index].jwks_url}));
  for (const source of after.definition.sources)
    if (source.admission.bot_messages === 'deny') delete source.admission.bot_messages;
  after.version++;
  return after;
}

test('channel updates compare exact saved policy, keeping provider, credential hashes and owner intact', () => {
  const {record} = fixture(), draft = editableChannel(record);
  assert.equal(draft.credentials, undefined); assert.equal(draft.secrets, undefined);
  assert.equal(draft.sources[0].jwks_url, undefined); assert.equal(draft.provider.binding_id, undefined);
  draft.sources[0].admission = {type: 'scoped', senders: {type: 'open'}, chats: {type: 'open'},
    groups: 'mention', bot_messages: 'deny'};
  const after = updated(record, draft);
  assertChannelUpdate(ctx, record, after, draft);
  for (const change of [v => {v.version = record.version;}, v => {v.record_id = 'foreign';},
    v => {v.definition.owner.subject = 'foreign';}, v => {v.definition.secrets = {};},
    v => {v.definition.sources[0].admission.groups = 'all';}, v => {v.definition.reply.auto_final = false;},
    v => {v.provider.binding_id = 'other';}, v => {v.definition.profile = {};},
    v => {v.definition.sources[0].admission.bot_messages = 'native_mentions';}]) {
    const invalid = structuredClone(after); change(invalid);
    assert.throws(() => assertChannelUpdate(ctx, record, invalid, draft), /admission_update_/);
  }
});

function ingress() {
  const input = {senderId: 'sender', chatId: 'group', isBotSender: false, mentioned: true,
    replica: 'b', text: 'synthetic event'};
  const result = {eventId: 'event', conversationId: 'conversation', ingress: {status: 202,
    body: {status: 'accepted', event_id: 'event', conversation_id: 'conversation'}, receipt: receipt()},
  wire: {...input, isGroup: true, textDigest: digest(input.text)}};
  return {input, result};
}
test('HTTP202 alone cannot pass admission: exact disposition, refusal reason and actual wire are required', () => {
  const {input, result} = ingress(); assertIngress(ctx, result, input);
  const refused = structuredClone(result); refused.ingress.body = {status: 'refused', reason: 'sender_denied'};
  assertIngress(ctx, refused, input, 'sender_denied');
  assert.throws(() => assertIngress(ctx, refused, input), /exact_acceptance/);
  assert.throws(() => assertIngress(ctx, result, input, 'sender_denied'), /exact_refusal/);
  assert.throws(() => assertIngress(ctx, refused, input, 'chat_denied'), /exact_refusal/);
  for (const change of [v => {v.wire.senderId = 'other';}, v => {v.wire.chatId = 'other';},
    v => {v.wire.mentioned = false;}, v => {v.wire.isBotSender = true;}, v => {v.wire.textDigest = 'bad';},
    v => {v.ingress.receipt.am = 'b';}, v => {v.ingress.receipt.serving_after = undefined;},
    v => {v.ingress.receipt.replica = 'a';}, v => {v.ingress.body.event_id = 'other';}]) {
    const invalid = structuredClone(result); change(invalid);
    assert.throws(() => assertIngress(ctx, invalid, input), /admission_/);
  }
});

const first = 'EG024_' + '1'.repeat(32), second = 'EG024_' + '2'.repeat(32), foreign = 'EG024_' + '3'.repeat(32);
const event = {eventId: 'event', responseId: 'response', agentId: 'agent', conversationId: 'conversation', consumerId: 'default',
  destination: 'group', expectedSender: 'bot'};
test('provider context must independently show both retained history and absence of other contexts', () => {
  const capture = {...event, requestCount: 1, messages: [{role: 'system', content: 'Synthetic system instructions'},
    {role: 'user', content: 'Normalized content: ' + first}, {role: 'assistant', content: 'Synthetic reply'},
    {role: 'user', content: [{type: 'input_text', text: second}]}]};
  assertProviderContext(ctx, event, capture, [first, second]);
  for (const change of [v => {v.messages.splice(1, 1);}, v => {v.messages.push({role: 'user', content: foreign});},
    v => {v.agentId = 'other';}, v => {v.conversationId = 'other';}, v => {v.requestCount = 2;},
    v => {v.responseId = 'other';}, v => {v.messages = [];},
    v => {v.messages[3].content = [{type: 'opaque', data: foreign}];}]) {
    const invalid = structuredClone(capture); change(invalid);
    assert.throws(() => assertProviderContext(ctx, event, invalid, [first, second]), /admission_/);
  }
});

test('partition inspection uses the actual transport conversation and provider Agent mapping', () => {
  const {record} = fixture(), partition = {consumer_id: 'default', transport_conversation_id: 'conversation',
    mapping: {binding_id: 'binding', generation: 1, conversation_id: 'conversation', agent_id: 'agent'}};
  assertPartitionMapping(ctx, record, event, partition);
  for (const change of [v => {v.transport_conversation_id = 'other';}, v => {v.consumer_id = 'other';},
    v => {v.mapping.binding_id = 'other';}, v => {v.mapping.agent_id = 'other';},
    v => {v.mapping.conversation_id = 'other';}, v => {v.mapping.generation = 0;}]) {
    const invalid = structuredClone(partition); change(invalid);
    assert.throws(() => assertPartitionMapping(ctx, record, event, invalid), /actual_partition/);
  }
  assert.throws(() => assertPartitionMapping(ctx, record, event,
    {grouping: 'conversation', conversations: ['predicted-a', 'predicted-b']}), /actual_partition/);
});

test('reply captures require the original destination, exact ack/final policy, content and one send', () => {
  const expected = [{step: 'ack', contentDigest: digest('ack'), ackAfterMs: 100},
    {step: 'answer', contentDigest: digest('answer')}];
  const effects = expected.map((want, index) => ({...event, sender: 'bot', platform: 'telegram', sink: 'source',
    step: want.step, contentDigest: want.contentDigest, deliveryId: 'delivery-' + index,
    status: 'delivered', attempts: 1, elapsedSinceAcceptedMs: 105}));
  assertReplyEffects(ctx, event, effects, expected);
  assertReplyEffects(ctx, event, [], []);
  assert.throws(() => assertReplyEffects(ctx, event, effects, []), /reply_count/);
  for (const change of [v => {v[0].elapsedSinceAcceptedMs = 99;}, v => {v[0].destination = 'other-group';},
    v => {v[0].sender = 'other-bot';}, v => {v[0].step = 'answer';}, v => {v[1].attempts = 2;},
    v => {v[1].deliveryId = v[0].deliveryId;}, v => {v[1].contentDigest = digest('wrong');},
    v => {v[1].status = 'accepted';}, v => {v[1].sink = 'result_webhook';},
    v => {v[1].responseId = 'other';}, v => {v.reverse();}]) {
    const invalid = structuredClone(effects); change(invalid);
    assert.throws(() => assertReplyEffects(ctx, event, invalid, expected), /admission_/);
  }
});

test('negative barriers and final drains reject late dispatch, pending, unknown and duplicate effects', () => {
  const counts = {turns: 10, effects: 10, deliveries: 13, pending: 0, unknown: 0, lateEffects: 0};
  assertAdmissionCounters(ctx, counts, 10, 13);
  for (const key of Object.keys(counts)) {
    assert.throws(() => assertAdmissionCounters(ctx, {...counts, [key]: counts[key] + 1}, 10, 13), /settled_exact/);
    const missing = {...counts}; delete missing[key];
    assert.throws(() => assertAdmissionCounters(ctx, missing, 10, 13), /settled_exact/);
  }
});

async function guarded(options = {}) {
  const before = fixture().record, draft = editableChannel(before);
  draft.reply = {...draft.reply, auto_final: false};
  const cleanups = [], calls = [], receipts = [], controller = new AbortController(); let handler;
  const page = {route: async (_match, fn) => {handler = fn;}, unroute: async () => {calls.push('unroute');}};
  const session = {page, origin: 'http://127.0.0.1:1234', networkIdle: async () => {}, egEvidence: async () => receipts};
  const local = {...ctx, signal: controller.signal, defer: (_label, fn) => cleanups.push(fn)};
  const guard = await guardChannelUpdates(local, session);
  const key = randomUUID();
  const dispatch = changes => handler({request: () => ({method: () => changes?.method ?? 'PUT',
    url: () => session.origin + (changes?.path ?? '/api/event-gateway/v1/channels/channel'),
    postDataJSON: () => changes?.body ?? {...draft, expected_version: before.version},
    allHeaders: async () => ({'idempotency-key': changes?.key ?? key})}),
  fetch: async settings => {
    calls.push('fetch'); assert.equal(settings.maxRetries, 0); assert.equal(settings.maxRedirects, 0);
    if (options.hold) await options.hold;
    if (options.fetchFailure) throw Error('synthetic_unknown');
    receipts.push(receipt('update', 'PUT', 200));
    return {status: () => 200, json: async () => updated(before, draft), headers: () => ({'x-request-id': 'update'}),
      dispose: async () => {calls.push('dispose'); if (options.disposeFailure) throw Error('synthetic_dispose');}};
  }, fulfill: async () => {calls.push('fulfill');}, abort: async () => {calls.push('abort');},
  fallback: async () => {calls.push('fallback');}});
  return {before, draft, guard, dispatch, calls, controller, cleanup: () => cleanups[0]()};
}

test('real UI updates forward once after exact intent validation and retain the original EG receipt', async () => {
  const f = await guarded();
  const result = await f.guard.save(f.before, f.draft, () => f.dispatch());
  assert.equal(result.record.version, 2); assert.equal(result.receipt.replica, 'b');
  assert.deepEqual(f.calls, ['fetch', 'fulfill', 'dispose']); await f.cleanup();
});

test('wrong revision, credential injection and unrelated browser mutations are stopped before dispatch', async () => {
  for (const changes of [{body: {expected_version: 99}}, {method: 'DELETE'}, {key: ''},
    {path: '/api/event-gateway/v1/channels/foreign'}]) {
    const f = await guarded();
    await assert.rejects(f.guard.save(f.before, f.draft, () => f.dispatch(changes)), /mutation_failed/);
    assert.equal(f.calls.includes('fetch'), false); await assert.rejects(f.cleanup(), /cleanup_failed/);
  }
  const f = await guarded();
  await assert.rejects(f.guard.save(f.before, f.draft, () => f.dispatch({body: {...f.draft,
    expected_version: 1, credentials: {}}})), /mutation_failed/);
  assert.equal(f.calls.includes('fetch'), false); await assert.rejects(f.cleanup(), /cleanup_failed/);
});

test('duplicates after a successful save still prevent PASS and cannot create another effect', async () => {
  const f = await guarded(); await f.guard.save(f.before, f.draft, () => f.dispatch());
  await f.dispatch(); assert.equal(f.calls.filter(call => call === 'fetch').length, 1);
  await assert.rejects(f.cleanup(), /cleanup_failed/);
});

test('unknown update outcome never retries; cleanup joins pending dispatch and preserves disposal failures', async () => {
  const lost = await guarded({fetchFailure: true});
  await assert.rejects(lost.guard.save(lost.before, lost.draft, () => lost.dispatch()), /mutation_failed/);
  assert.equal(lost.calls.filter(call => call === 'fetch').length, 1);
  await assert.rejects(lost.cleanup(), /cleanup_failed/);
  let release; const hold = new Promise(resolve => {release = resolve;});
  const pending = await guarded({hold, disposeFailure: true});
  const saving = pending.guard.save(pending.before, pending.draft, () => pending.dispatch());
  const failed = assert.rejects(saving, /mutation_failed/);
  let cleaned = false;
  const cleaning = assert.rejects(pending.cleanup(), /cleanup_failed/).then(() => {cleaned = true;});
  await new Promise(resolve => setImmediate(resolve)); assert.equal(cleaned, false);
  release(); await failed; await cleaning;
  assert.equal(pending.calls.filter(call => call === 'fetch').length, 1);
  assert.equal(pending.calls.includes('abort'), false, 'do not abort a fulfilled response after disposal failure');
});

test('cancellation of an unsubmitted update settles its waiter and forwards no mutation', async () => {
  const f = await guarded();
  const saving = f.guard.save(f.before, f.draft, async () => {});
  f.controller.abort(); await assert.rejects(saving, /update_cancelled/);
  assert.equal(f.calls.includes('fetch'), false); await f.cleanup();
});

test('named admission setup blocks without independent history, effects, hold release or finality primitives', async () => {
  const base = {...fixture(), local: true, synthetic: true, replicas: [{ready: true}, {ready: true}], close: async () => {}};
  const methods = ['request', 'platformSend', 'counters', 'waitForTurn', 'providerContext', 'waitForDelivery',
    'captureEffects', 'releaseCompletion', 'quiesce', 'drain'];
  for (const name of methods) base[name] = async () => ({});
  for (const missing of methods) {
    const partial = {...base}; delete partial[missing]; const cleanups = [];
    await assert.rejects(parentEgSetup({row: {}, defer: (_label, cleanup) => cleanups.push(cleanup)},
      {eg: {acquire: async () => partial}}, 'channel_admission'), error => error instanceof Blocked);
    assert.equal(cleanups.length, 1); await cleanups[0]();
  }
});
