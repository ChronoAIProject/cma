import test from 'node:test';
import assert from 'node:assert/strict';
import {randomBytes, randomUUID} from 'node:crypto';
import {Blocked} from '../../runner.mjs';
import {digest} from './helpers.mjs';
import {setups} from './setup.mjs';
import {nativeDiscordCase, discordIntents, discordReactionEventId, validateNativeDiscordFixture,
  assertDiscordSource, assertDiscordGateway, assertDiscordDispatch, assertDiscordAssociation,
  assertDiscordDelivery, assertDiscordActivity, assertDiscordProducerUI} from './native-discord.mjs';

const ctx = {assert: (v, code) => assert.ok(v, code)};
const at = n => new Date(Date.UTC(2026, 10, 17) + n).toISOString();
const e = {ownerSubject: 'owner', consumerId: 'default', botId: '100', inactiveBotId: '101', senderId: '200',
  reactorId: '201', botSenderId: '202', privateChat: '300', serverChat: '301', guildId: '400',
  inactiveRecordId: 'inactive-record', inactiveChannelId: 'inactive-channel',
  credentialSha256: digest(randomBytes(32)), sourceRevision: 'sha256:' + digest('source-revision')};
const source = 'a'.repeat(40), image = 'eg@sha256:' + 'b'.repeat(64), session = randomUUID();
function fixture() {
  return {local: true, synthetic: true, recordId: 'record', channelId: 'channel', expected: {...e},
    replicas: ['a', 'b'].map(slot => ({slot, instance: 'eg-' + slot, image, ready: true})),
    discordTransport: {schema: 'multi-eg-discord-local-binding-v1', source, image, artifact_sha256: digest('binding'),
      rest_origin: 'http://127.0.0.1:12345', gateway_origin: 'ws://127.0.0.1:12346'},
    ...Object.fromEntries(['request', 'gatewaySnapshot', 'gatewayDispatch', 'gatewayCapture', 'gatewayCheckpoint', 'waitForTurn',
      'nativeEventCapture', 'providerContext', 'waitForDelivery', 'captureEffects', 'counters', 'drain', 'close']
      .map(k => [k, async () => {}]))};
}
function record(replica = 'a', inactive = false) {
  return {record_id: inactive ? e.inactiveRecordId : 'record', channel_id: inactive ? e.inactiveChannelId : 'channel',
    version: 1, endpoints: {custom_event_url: 'https://gateway.example/custom', nyxid_callback_url: 'https://gateway.example/relay'},
    provider: {slug: 'cma_codex'}, definition: {name: inactive ? 'Inactive fixture' : 'Active fixture', creation: {ready: true}, owner: {subject: 'owner'},
      sources: [{type: 'discord', bot_id: inactive ? '101' : '100', intents: discordIntents,
        admission: {type: 'scoped', senders: {type: 'allowlist', ids: ['200', '201', '202']},
          chats: {type: 'allowlist', ids: ['300', '301']}, groups: 'mention'}}],
      partition: {version: 1, key: 'conversation_and_sender'},
      reply: {auto_final: true, ack_after_ms: 0, sinks: [{type: 'source'}]},
      routing: {version: 1, default_consumer: {name: 'default'}, default_targets: ['default']}},
    native_discord_activation: {state: inactive ? 'not_selected' : 'selected', detail: inactive ? 'Not selected.' : 'Selected; connection is not confirmed.',
      capacity: 8, selected_records: 1,
      connection: {scope: 'serving_instance', connected: !inactive && replica === 'a',
        authenticated: !inactive && replica === 'a', ready: !inactive && replica === 'a', stale: false,
        lease: replica === 'a' ? 'acquired' : 'contended', identify: replica === 'a' ? 'sent' : 'not_attempted',
        last_ready_at: at(5), observed_at: at(10), valid_until: at(30000),
        last_dispatch_outcome: 'refused', last_dispatch_at: at(80)}}};
}
const caps = {relay_authoring: {native_discord_writes_enabled: true, native_discord_reactions_enabled: true,
  native_discord_auto_activation_enabled: false, native_discord_capacity: 8}};
function serving(replica = 'a', n = 0) {
  return {schema: 'c275-eg-serving-v1', scope: 'eg', replica, origin: 'http://127.0.0.1:' + (replica === 'a' ? 12347 : 12348),
    pod_uid: 'pod-' + replica, instance_id: 'eg-' + replica, container_id: 'container-' + replica,
    source, image, ready: true, observed_at: at(n)};
}
function read(replica = 'a') {
  return {body: record(replica), eg_receipt: {scope: 'eg', replica, request_id: 'read-' + replica, method: 'GET', status: 200,
    serving: serving(replica, 9), serving_after: serving(replica, 13), started_at: at(10), ended_at: at(12),
    attribution: {verdict: 'PASS', basis: 'pod_forward_before_after'}}};
}
function gateway() {
  return {schema: 'multi-eg-discord-gateway-v1', artifact_sha256: digest('gateway'),
    monitored_bot_ids: ['100', '101'], sessions_complete: true, observed_from: at(0), observed_until: at(100),
    sessions: [{schema: 'multi-eg-discord-socket-v1', socket_id: 'socket', session_id: session,
      record_id: 'record', bot_id: '100', source_revision: e.sourceRevision, artifact_sha256: digest('socket'),
      replica: 'a', process_before: serving('a', 0), process_after: serving('a', 100), started_at: at(1), ended_at: at(99),
      attribution: {basis: 'process_socket_before_after', verdict: 'PASS', artifact_sha256: digest('attribution')}}],
    discovery: [{path: '/api/v10/users/@me', response: {id: '100', bot: true}},
      {path: '/api/v10/gateway/bot', response: {url: 'wss://gateway.discord.gg/', shards: 1,
        session_start_limit: {total: 100, remaining: 99, reset_after: 10000, max_concurrency: 1}}}]
      .map((v, i) => ({...v, bot_id: '100', method: 'GET', status: 200, request_id: 'discovery-' + i, artifact_sha256: digest(i)})),
    activation: {socket_id: 'socket', artifact_sha256: digest('activation'), hello: {op: 10, d: {heartbeat_interval: 10000}},
      identify: {op: 2, d: {intents: discordIntents, token_sha256: e.credentialSha256}},
      ready: {op: 0, t: 'READY', s: 1, d: {user: {id: '100', bot: true}, session_id: session}},
      heartbeat: {op: 1, d: 1}, heartbeatAck: {op: 11}}};
}
const input = {name: 'MESSAGE_CREATE', completionText: 'answer', data: {id: '500', channel_id: '301', guild_id: '400',
  author: {id: '200', bot: false}, content: 'synthetic server message'}};
const event = {eventId: 'discord:100:500', responseId: 'response', conversationId: 'conversation', agentId: 'agent',
  consumerId: 'default', eventRef: randomUUID(), destination: '301', expectedSender: '100', expectedContentDigest: digest('answer')};
function wire(request = input, sequence = 2) {
  return {schema: 'multi-eg-discord-dispatch-v1', socket_id: 'socket', session_sha256: digest(session),
    artifact_sha256: digest('wire-' + sequence), frame: {op: 0, t: request.name, s: sequence, d: structuredClone(request.data)},
    sent_at: at(20), heartbeat: {observed_at: at(21), socket_id: 'socket', frame: {op: 1, d: sequence}, artifact_sha256: digest('heartbeat')}};
}
function checkpoint(sequence = 2) {
  return {schema: 'multi-eg-discord-checkpoint-v1', seal_verified: true, artifact_sha256: digest('checkpoint'),
    record_id: 'record', bot_id: '100', source_revision: e.sourceRevision, session_sha256: digest(session),
    sequence, observed_at: at(22)};
}
function original(ev = event, request = input) {
  return {schema: 'multi-eg-native-event-v1', ...ev, seal_verified: true, artifact_sha256: digest('provider'),
    request_id: 'am-request', event_ref_sha256: digest(ev.eventRef), source: {type: 'discord', bot_id: '100'},
    actor_id: '200', actor_kind: 'human', text_sha256: digest(request.data.content),
    reply: {type: 'discord', bot_id: '100', channel_id: request.data.channel_id, message_id: request.data.id, thread_id: null}};
}
function delivery(ev = event, request = input) {
  return {eventId: ev.eventId, responseId: ev.responseId, destination: ev.destination, sender: '100',
    contentDigest: ev.expectedContentDigest, count: 1, status: 'delivered', platformReceipt: {
      schema: 'multi-eg-discord-delivery-v1', request_id: 'send-' + ev.eventId, artifact_sha256: digest('delivery'),
      method: 'POST', path: '/api/v10/channels/' + request.data.channel_id + '/messages', status: 200, bot_id: '100',
      request: {content_sha256: ev.expectedContentDigest, allowed_mentions: {parse: [], replied_user: false},
        message_reference: {message_id: request.data.message_id ?? request.data.id, channel_id: request.data.channel_id, fail_if_not_exists: false}},
      response: {id: '600', channel_id: request.data.channel_id, author: {id: '100', bot: true}}}};
}
function reaction() {
  const request = {name: 'MESSAGE_REACTION_ADD', data: {message_id: '500', channel_id: '301', guild_id: '400',
    user_id: '201', emoji: {id: null, name: '👍'}, type: 0, burst: false}};
  const ev = {...event, eventId: discordReactionEventId(fixture(), session, 3), responseId: 'reaction-response'};
  const capture = {...original(), ...ev, actor_id: '201', cloudevent: {specversion: '1.0', id: ev.eventId,
    source: 'urn:cmaeg:discord:100', type: 'cma.message.reaction.updated.v1', subject: '500', datacontenttype: 'application/json',
    data: {message_id: '500', original_event_id: event.eventId, target_kind: 'inbound', target_author: {id: '200', kind: 'human'},
      operation: 'add', representation: 'delta', emoji: {id: null, name: '👍'}, reaction_type: 0, burst: false}}};
  return {request, ev, capture};
}

test('Discord named setup blocks unsupported transports and retains cleanup custody', async () => {
  assert.equal(nativeDiscordCase.id, 'C275:EG-040');
  assert.deepEqual(nativeDiscordCase.steps, ['step_1', 'step_2', 'expected_result']);
  assert.deepEqual(nativeDiscordCase.fixtures, ['browser', 'user-fixtures', 'eg_browser']);
  validateNativeDiscordFixture(fixture());
  for (const edit of [v => delete v.discordTransport, v => v.discordTransport.gateway_origin = 'wss://gateway.discord.gg/',
    v => delete v.gatewayCapture, v => delete v.gatewayCheckpoint, v => delete v.nativeEventCapture, v => v.expected.reactorId = '200',
    v => v.expected.sourceRevision = digest('unprefixed')]) {
    const lease = fixture(), cleanups = []; let closed = 0; lease.close = async () => closed++; edit(lease);
    await assert.rejects(setups.native_discord({...ctx, row: {case_id: nativeDiscordCase.id},
      defer: (_label, fn) => cleanups.push(fn)}, {eg: {acquire: async () => lease}}), error => error instanceof Blocked);
    await cleanups[0](); await cleanups[0](); assert.equal(closed, 1);
  }
});

test('Discord source inspection enforces native intents, scoped admission, routing and real capacity', () => {
  for (const replica of ['a', 'b']) for (const inactive of [false, true])
    assertDiscordSource(ctx, fixture(), record(replica, inactive), caps, inactive);
  for (const edit of [r => r.definition.sources[0].intents &= ~(1 << 15), r => r.definition.sources[0].type = 'nyxid_relay',
    r => r.definition.sources[0].admission.senders = {type: 'open'}, r => delete r.definition.routing,
    r => r.definition.routing.default_targets = ['foreign'], r => r.native_discord_activation.selected_records = 9]) {
    const r = record(); edit(r); assert.throws(() => assertDiscordSource(ctx, fixture(), r, caps), /native_discord_/);
  }
});

test('selection alone cannot prove ready; one real EG socket and a standby peer are valid', () => {
  const reads = [read('a'), read('b')];
  assertDiscordGateway(ctx, fixture(), gateway(), reads);
  for (const edit of [g => g.sessions[0].process_after.instance_id = 'different',
    g => g.sessions[0].process_before.source = 'c'.repeat(40), g => g.sessions[0].replica = 'b',
    g => g.sessions[0].attribution.basis = 'requested_replica', g => g.sessions[0].ended_at = at(-1),
    g => g.sessions.push({...g.sessions[0], bot_id: '101'}), g => g.sessions_complete = false]) {
    const g = gateway(); edit(g); assert.throws(() => assertDiscordGateway(ctx, fixture(), g, reads), /native_discord_/);
  }
  for (const edit of [c => c.ready = null, c => c.stale = true, c => c.lease = 'contended', c => c.valid_until = at(1)]) {
    const changed = [read('a'), read('b')]; edit(changed[0].body.native_discord_activation.connection);
    assert.throws(() => assertDiscordGateway(ctx, fixture(), gateway(), changed), /actual_health/);
  }
});

test('native discovery must match bot, budget and captured IDENTIFY/READY session', () => {
  for (const edit of [g => g.discovery[0].response.id = '101', g => g.discovery[1].response.shards = 2,
    g => g.discovery[1].response.session_start_limit.remaining = 0, g => g.activation.identify.d.intents &= ~(1 << 15),
    g => g.activation.ready.d.session_id = randomUUID(), g => g.activation.heartbeatAck.op = 1,
    g => g.activation.identify.d.token_sha256 = digest(randomBytes(32))]) {
    const g = gateway(); edit(g); assert.throws(() => assertDiscordGateway(ctx, fixture(), g, [read('a'), read('b')]), /native_discord_/);
  }
});

test('Gateway heartbeat proves receipt, while an independently verified journal proves durable commit', () => {
  const socket = gateway().sessions[0]; assertDiscordDispatch(ctx, socket, wire(), input, 2, checkpoint());
  for (const edit of [w => w.frame.d.author.bot = true, w => w.frame.d.channel_id = '302', w => w.frame.t = 'MESSAGE_UPDATE',
    w => w.socket_id = 'other', w => w.session_sha256 = digest('other'), w => w.heartbeat.frame.d = 1,
    w => w.heartbeat.observed_at = at(0), w => delete w.heartbeat.artifact_sha256]) {
    const w = wire(); edit(w); assert.throws(() => assertDiscordDispatch(ctx, socket, w, input, 2, checkpoint()), /native_discord_/);
  }
  assert.throws(() => assertDiscordDispatch(ctx, socket, wire(), input, 2), /durable_journal_checkpoint/);
  for (const edit of [c => c.sequence = 1, c => c.seal_verified = false, c => c.record_id = 'other',
    c => c.session_sha256 = digest('other'), c => c.source_revision = 'sha256:' + digest('other'),
    c => c.observed_at = at(0)]) {
    const c = checkpoint(); edit(c);
    assert.throws(() => assertDiscordDispatch(ctx, socket, wire(), input, 2, c), /durable_journal_checkpoint/);
  }
});

test('native message and reaction evidence bind real AM capture, retained target and original partition', () => {
  const turn = {amRequests: [{request_id: 'am-request'}]};
  assertDiscordAssociation(ctx, fixture(), original(), event, input, turn);
  const {request, ev, capture} = reaction(); assertDiscordAssociation(ctx, fixture(), capture, ev, request, turn, event);
  for (const edit of [c => c.seal_verified = false, c => c.request_id = 'foreign', c => c.reply.message_id = '501',
    c => c.cloudevent.time = at(0), c => c.cloudevent.data.original_event_id = 'other',
    c => c.cloudevent.data.new_reactions = [], c => c.cloudevent.data.target_kind = 'outbound', c => c.actor_kind = 'unknown']) {
    const c = structuredClone(capture); edit(c);
    assert.throws(() => assertDiscordAssociation(ctx, fixture(), c, ev, request, turn, event), /native_discord_/);
  }
  assert.throws(() => assertDiscordAssociation(ctx, fixture(), capture, ev, request, turn,
    {...event, conversationId: 'reactor-partition'}), /original_conversation/);
  assert.notEqual(discordReactionEventId(fixture(), session, 3), discordReactionEventId(fixture(), session, 4));
  assert.notEqual(discordReactionEventId(fixture(), session, 3), discordReactionEventId(fixture(), randomUUID(), 3));
});

test('native delivery proves bot, exact original-message quote and safe mentions', () => {
  assertDiscordDelivery(ctx, fixture(), delivery(), event, input);
  for (const edit of [d => delete d.platformReceipt.request.message_reference,
    d => d.platformReceipt.request.message_reference.message_id = 'wrong',
    d => d.platformReceipt.request.allowed_mentions.parse = ['everyone'],
    d => d.platformReceipt.request.content_sha256 = digest('wrong'), d => d.platformReceipt.response.author.id = '101',
    d => d.platformReceipt.path = '/api/v10/channels/302/messages', d => d.count = 2]) {
    const d = delivery(); edit(d); assert.throws(() => assertDiscordDelivery(ctx, fixture(), d, event, input));
  }
});

function page(turns = []) {
  return {cursor: 'cursor', retention_days: 7, limit: 100, has_more: false, items: turns.map(({event: ev, input: request}) => ({
    event_id: ev.eventId, source: {type: 'discord', bot_id: '100'}, actor: {id: request.name === 'MESSAGE_REACTION_ADD' ? '201' : '200', kind: 'human'},
    conversation: {id: request.data.channel_id, kind: request.data.guild_id ? 'group' : 'private'}, received_at: at(20), occurred_at: at(20),
    consumers: [{consumer_id: 'default', agent_id: ev.agentId, response_id: ev.responseId, conversation_id: 'provider-conversation',
      state: 'completed', dispatched_at: at(22), accepted_at: at(23), produced_at: at(24), completed_at: at(25),
      deliveries: [{step: 'answer', outcome: 'confirmed', observed_at: at(24), confirmed_at: at(25)}]}]}))};
}

function browserFixture(lease, changes = {}) {
  let replica = 'a', dialog = false;
  const locator = (role, name) => ({
    getByRole: (r, options = {}) => locator(r, options.name), getByLabel: label => locator('input', label),
    getByText: name => locator('text', name), filter() {return this;},
    async count() {return name === 'Messages from bots' ? (changes.botControl ? 1 : 0) : role === 'dialog' ? Number(dialog) : 1;},
    async waitFor() {}, async isVisible() {
      if (role === 'dialog') return dialog;
      if (role === 'text' && String(name).includes('Bot messages')) return !changes.missingNotice;
      return true;
    },
    async innerText() {return changes.wrongStatus ? 'Unrelated status' : record(replica).native_discord_activation.detail;},
    async click() {
      if (name === 'Producer: Discord (Direct Discord bot)') dialog = true;
      if (name === 'Close') dialog = false;
    },
    async getAttribute() {return 'true';}, async inputValue() {return changes.wrongBot ? '101' : '100';},
    async isChecked() {return name !== changes.unchecked;},
  });
  const page = {getByRole: (r, o = {}) => locator(r, o.name), getByText: name => locator('text', name),
    async goto() {}, url: () => 'http://127.0.0.1:12350/event-gateway',
    async route() {}, async unroute() {}, async screenshot() {return Buffer.from('ui');},
    async waitForResponse(predicate) {
      const response = {status: () => 200, url: () => 'http://127.0.0.1:12350/api/event-gateway/v1/channels',
        request: () => ({method: () => 'GET'}), headers: () => ({'x-request-id': 'read-' + replica}),
        json: async () => ({items: [record(replica)]})};
      assert.equal(predicate(response), true); return response;
    }};
  const session = {page, origin: 'http://127.0.0.1:12350', async close() {}};
  return {page, fixture: name => {
    if (name === 'user-fixtures') return {take: async () => lease};
    if (name === 'browser') return {open: async () => session, evidence: async () => ({real_ui: true,
      secrets_redacted: true, compiled_source: source, requests: [{am: 'a', request_id: 'browser-request'}]})};
    assert.equal(name, 'eg_browser');
    return {prepare: async () => ({close: async () => {}, networkIdle: async () => {},
      switchEG: async next => {replica = next;}, egEvidence: async () => [read(replica).eg_receipt]})};
  }};
}

test('compiled producer inspection requires truthful native bot notice and saved intent controls', async () => {
  await assertDiscordProducerUI(ctx, browserFixture(fixture()).page, record());
  for (const change of [{missingNotice: true}, {botControl: true}, {wrongStatus: true}, {wrongBot: true},
    {unchecked: 'Receive reactions to retained messages'}])
    await assert.rejects(assertDiscordProducerUI(ctx, browserFixture(fixture(), change).page, record()), /native_discord_/);
});

test('retained Activity rejects missing reaction, another consumer and unconfirmed delivery', () => {
  const turns = [{event, input}]; assertDiscordActivity(ctx, fixture(), page(turns), turns);
  for (const edit of [p => p.items.pop(), p => p.items[0].consumers[0].consumer_id = 'foreign',
    p => p.items[0].actor.kind = 'bot', p => p.items[0].consumers[0].deliveries[0].outcome = 'unknown']) {
    const p = page(turns); edit(p); assert.throws(() => assertDiscordActivity(ctx, fixture(), p, turns));
  }
});

test('Discord case never retries an uncertain Gateway dispatch and releases its own lease', async () => {
  const lease = fixture(), cleanups = []; let sends = 0, closed = 0, requests = 0;
  lease.close = async () => closed++;
  lease.request = async ({path, replica}) => {
    const r = read(replica); r.eg_receipt.request_id = 'request-' + requests++;
    r.body = path.endsWith('/activity') ? page() : path.endsWith('channel-capabilities') ? caps : record(replica, path.includes('inactive-channel'));
    return {...r, status: 200, requestId: r.eg_receipt.request_id, instance: 'eg-' + replica};
  };
  lease.gatewaySnapshot = async () => gateway();
  lease.counters = async () => ({turns: 0, effects: 0, deliveries: 0, pending: 0, unknown: 0, lateEffects: 0});
  lease.gatewayDispatch = async () => {sends++; throw Error('discord_gateway_dispatch_unknown');};
  await assert.rejects(nativeDiscordCase.run({...ctx, row: {id: 'EG-040', case_id: nativeDiscordCase.id}, am: 'a',
    step: async (_label, fn) => fn(), observe: async (_label, fn) => fn(), defer: (_label, fn) => cleanups.push(fn),
    fixture: browserFixture(lease).fixture}), /discord_gateway_dispatch_unknown/);
  for (const cleanup of cleanups.reverse()) await cleanup();
  assert.equal(sends, 1); assert.equal(closed, 1);
});

test('offline case orchestration preserves native reaction context and peer-dispatched bot refusal', async () => {
  const lease = fixture(), cleanups = [], completed = [], captures = new Map(), events = new Map(), observations = [];
  let sequence = 1, closed = 0;
  lease.close = async () => closed++;
  lease.request = async ({path, replica}) => {
    const r = read(replica);
    r.body = path.endsWith('/activity') ? page(path.includes('inactive-channel') ? [] : completed)
      : path.endsWith('channel-capabilities') ? caps : record(replica, path.includes('inactive-channel'));
    // Inbox work may finish on the standby EG, distinct from the socket owner.
    if (replica === 'a' && r.body.native_discord_activation) r.body.native_discord_activation.connection.last_dispatch_outcome = 'unknown';
    return {...r, status: 200, requestId: r.eg_receipt.request_id, instance: 'eg-' + replica};
  };
  lease.gatewaySnapshot = async () => gateway();
  lease.gatewayDispatch = async request => {
    const s = ++sequence; captures.set(s, wire(request, s));
    if (!request.data.author?.bot) {
      const reaction = request.name === 'MESSAGE_REACTION_ADD';
      const eventId = reaction ? discordReactionEventId(lease, session, s) : 'discord:100:' + request.data.id;
      const privateChat = request.data.channel_id === e.privateChat, am = privateChat ? 'a' : 'b';
      const ev = {...event, eventId, eventRef: randomUUID(), responseId: 'response-' + s, agentId: 'agent-' + request.data.channel_id,
        conversationId: 'conversation-' + request.data.channel_id, destination: request.data.channel_id,
        expectedContentDigest: digest(request.completionText)};
      const serving = {schema: 'c275-serving-v1', am, origin: 'http://127.0.0.1:12349', ready: true,
        pod_uid: 'am-pod-' + am, instance_id: 'am-' + am, container_id: 'am-container-' + am, observed_at: at(0)};
      const turn = {...ev, servingAm: am, owningAm: am === 'a' ? 'b' : 'a', epoch: 3,
        providerAccepted: true, turnCount: 1, effectCount: 1, amRequests: [{am, request_id: 'am-request', status: 202, elapsed_ms: 1,
          agent_id_sha256: digest(ev.agentId), serving, serving_after: {...serving, observed_at: at(3)},
          started_at: at(1), ended_at: at(2), attribution: {verdict: 'PASS', basis: 'pod_forward_before_after'},
          owner_proof: {verdict: 'PASS', kind: 'non_owner', agent_sha256: digest(ev.agentId), request_id: 'am-request',
            ingress: am, owner: am === 'a' ? 'b' : 'a', owner_epoch: 3,
            ingress_instance_sha256: digest(serving.instance_id), owner_instance_sha256: digest('peer-' + am)}}]};
      let capture;
      if (reaction) {
        const base = completed[1].event;
        capture = {...original(ev, {data: {...request.data, id: request.data.message_id, content: ''}}), actor_id: '201',
          cloudevent: {specversion: '1.0', id: ev.eventId, type: 'cma.message.reaction.updated.v1', source: 'urn:cmaeg:discord:100',
            subject: request.data.message_id, datacontenttype: 'application/json', data: {message_id: request.data.message_id,
              original_event_id: base.eventId, target_kind: 'inbound', target_author: {id: '200', kind: 'human'},
              operation: 'add', representation: 'delta', emoji: request.data.emoji, reaction_type: 0, burst: false}}};
      } else capture = original(ev, request);
      events.set(eventId, {event: ev, input: request, turn, capture});
    }
    return {socket_id: 'socket', sequence: s};
  };
  lease.gatewayCapture = async ({sequence: s}) => captures.get(s);
  lease.gatewayCheckpoint = async ({sequence: s}) => checkpoint(s);
  lease.waitForTurn = async ({eventId}) => events.get(eventId).turn;
  lease.nativeEventCapture = async ({eventId}) => events.get(eventId).capture;
  lease.providerContext = async ({eventId}) => {
    const {event: ev, input: request, capture} = events.get(eventId);
    return {...ev, requestCount: 1, messages: [{role: 'user', content: capture.cloudevent
      ? 'Event ' + capture.cloudevent.type + ': ' + JSON.stringify(capture.cloudevent.data) : request.data.content}]};
  };
  lease.waitForDelivery = async ({eventId}) => {
    const t = events.get(eventId); completed.push(t); return delivery(t.event, t.input);
  };
  lease.captureEffects = async () => completed.map(t => ({eventId: t.event.eventId, responseId: t.event.responseId,
    step: 'answer', attempts: 1, platformRequestId: 'send-' + t.event.eventId}));
  lease.counters = lease.drain = async () => ({turns: completed.length, effects: completed.length,
    deliveries: completed.length, pending: 0, unknown: 0, lateEffects: 0});
  const steps = [];
  await nativeDiscordCase.run({...ctx, row: {id: 'EG-040', case_id: nativeDiscordCase.id}, am: 'a',
    step: async (label, fn) => {steps.push(label); await fn();},
    observe: async (label, fn) => {const v = await fn(); observations.push({label, value: v}); return v;},
    defer: (_label, fn) => cleanups.push(fn), fixture: browserFixture(lease).fixture});
  for (const cleanup of cleanups.reverse()) await cleanup();
  assert.deepEqual(steps, nativeDiscordCase.steps); assert.equal(sequence, 5); assert.equal(completed.length, 3); assert.equal(closed, 1);
  assert.equal(completed[2].event.agentId, completed[1].event.agentId);
  assert.deepEqual(new Set(observations.filter(o => o.label === 'eg_am_evidence').flatMap(o => o.value.requests.map(r => r.am))), new Set(['a', 'b']));
  assert.equal(JSON.stringify(observations).includes(session), false);
  for (const t of completed) assert.equal(JSON.stringify(observations).includes(t.event.eventRef), false);
});
