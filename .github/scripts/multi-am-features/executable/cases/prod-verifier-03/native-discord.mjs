import {createHash, randomInt, randomUUID} from 'node:crypto';
import {Blocked} from '../../runner.mjs';
import {validEgReceipt} from '../../evidence-scope.mjs';
import {define, acquire, invoke, egRequest, peer, segment, same, digest, retainTurn, delivered,
  openBrowser, goto, visible, uiReceipt} from './helpers.mjs';
import {assertRelayCounters} from './relay-verification.mjs';
import {assertJourneyContext} from './relay-journey.mjs';
import {assertActivityPage} from './activity-observer.mjs';
import {openListedChannel, openChannelNode, closeChannelInspector} from './channel-browser.mjs';
import {guardDiscoveryWrites} from './channel-capabilities.mjs';

const text = v => typeof v === 'string' && v.length > 0;
const hash = v => /^[a-f0-9]{64}$/.test(v ?? '');
const id = v => typeof v === 'string' && /^[1-9][0-9]{0,19}$/.test(v);
const time = v => Number.isFinite(Date.parse(v));
export const discordIntents = 1 | (1 << 9) | (1 << 12) | (1 << 15) | (1 << 10) | (1 << 13);
const processKeys = ['schema', 'scope', 'replica', 'origin', 'pod_uid', 'instance_id', 'container_id', 'source', 'image'];
const sameProcess = (a, b) => processKeys.every(k => a?.[k] === b?.[k]);
const saved = r => {const {native_discord_activation, ...rest} = r; return rest;};

export function validateNativeDiscordFixture(lease) {
  const e = lease.expected, binding = lease.discordTransport;
  // Current native binaries have fixed Discord endpoints. A declaration that a
  // fake bot exists is insufficient: the parent must supply an admitted local
  // transport binding for discovery, Gateway and outbound REST on this image.
  if (binding?.schema !== 'multi-eg-discord-local-binding-v1'
    || !hash(binding.artifact_sha256) || !/^[a-f0-9]{40}$/.test(binding.source ?? '')
    || !/@sha256:[a-f0-9]{64}$/.test(binding.image ?? ''))
    throw new Blocked('native_discord_supported_local_transport_required');
  for (const key of ['rest_origin', 'gateway_origin']) {
    let u; try {u = new URL(binding[key]);} catch {throw new Blocked('native_discord_local_transport_origin_required');}
    if (!['127.0.0.1', '[::1]'].includes(u.hostname) || u.username || u.password || u.search || u.hash
      || u.pathname !== '/' || !(key === 'rest_origin' ? ['http:', 'https:'] : ['ws:', 'wss:']).includes(u.protocol))
      throw new Blocked('native_discord_local_transport_origin_required');
  }
  if (!text(lease.recordId) || !text(lease.channelId) || !e || e.consumerId !== 'default'
    || !['ownerSubject', 'inactiveRecordId', 'inactiveChannelId'].every(k => text(e[k]))
    || !['botId', 'inactiveBotId', 'senderId', 'botSenderId', 'reactorId', 'privateChat', 'serverChat', 'guildId'].every(k => id(e[k]))
    || new Set([e.botId, e.inactiveBotId, e.senderId, e.botSenderId, e.reactorId]).size !== 5
    || e.privateChat === e.serverChat || e.inactiveRecordId === lease.recordId
    || e.inactiveChannelId === lease.channelId || !hash(e.credentialSha256) || !/^sha256:[a-f0-9]{64}$/.test(e.sourceRevision ?? ''))
    throw new Blocked('native_discord_isolated_seed_required');
  for (const method of ['request', 'gatewaySnapshot', 'gatewayDispatch', 'gatewayCapture', 'gatewayCheckpoint', 'waitForTurn',
    'nativeEventCapture', 'providerContext', 'waitForDelivery', 'captureEffects', 'counters', 'drain'])
    if (typeof lease[method] !== 'function') throw new Blocked('native_discord_primitive_' + method.toLowerCase());
  return e;
}

export function assertDiscordSource(ctx, lease, record, capabilities, inactive = false) {
  const e = lease.expected, d = record?.definition, s = d?.sources?.[0], a = record?.native_discord_activation;
  ctx.assert(record?.record_id === (inactive ? e.inactiveRecordId : lease.recordId)
    && record.channel_id === (inactive ? e.inactiveChannelId : lease.channelId)
    && d?.owner?.subject === e.ownerSubject && d.creation?.ready === true && record.provider?.slug === 'cma_codex'
    && d.sources?.length === 1 && s.type === 'discord' && s.bot_id === (inactive ? e.inactiveBotId : e.botId)
    && s.intents === discordIntents, 'native_discord_exact_source_message_content_reaction_intents');
  ctx.assert(s.admission?.type === 'scoped'
    && same(s.admission.senders, {type: 'allowlist', ids: [e.senderId, e.reactorId, e.botSenderId]})
    && same(s.admission.chats, {type: 'allowlist', ids: [e.privateChat, e.serverChat]})
    && s.admission.groups === 'mention' && [undefined, 'deny'].includes(s.admission.bot_messages)
    && ['thread_history', 'thread_lifecycle', 'conversation_history'].every(k => s[k] !== true),
  'native_discord_native_admission_scope');
  const r = d.routing;
  ctx.assert(r?.version === 1 && same(r.default_consumer, {name: 'default'}) && same(r.default_targets, ['default'])
    && same(r.consumers ?? [], []) && same(r.rules ?? [], []) && r.default_reply === undefined
    && [undefined, 'auto'].includes(r.reply_label) && same(d.partition, {version: 1, key: 'conversation_and_sender'})
    && same(d.reply, {auto_final: true, ack_after_ms: 0, sinks: [{type: 'source'}]})
    && d.provider?.target?.type !== 'existing_agent' && d.profile?.environment?.session_affinity !== 'source_user',
  'native_discord_durable_default_consumer_policy');
  const c = capabilities?.relay_authoring;
  ctx.assert(c?.native_discord_writes_enabled === true && c.native_discord_reactions_enabled === true
    && c.native_discord_auto_activation_enabled === false && Number.isSafeInteger(c.native_discord_capacity)
    && c.native_discord_capacity > 0 && a?.capacity === c.native_discord_capacity
    && Number.isSafeInteger(a.selected_records) && a.selected_records > 0 && a.selected_records <= a.capacity
    && a.state === (inactive ? 'not_selected' : 'selected'), 'native_discord_actual_selection_capacity');
  ctx.assert(a.connection?.scope === 'serving_instance', 'native_discord_instance_scoped_connection');
  if (inactive) ctx.assert(a.connection.connected !== true && a.connection.authenticated !== true
    && a.connection.ready !== true, 'native_discord_unselected_source_has_no_connection');
}

// This is a socket observation, never a fabricated HTTP request or AM receipt.
export function assertDiscordSocket(ctx, lease, socket, reads) {
  ctx.assert(socket?.schema === 'multi-eg-discord-socket-v1' && text(socket.socket_id)
    && text(socket.session_id) && socket.record_id === lease.recordId && socket.bot_id === lease.expected.botId
    && socket.source_revision === lease.expected.sourceRevision && hash(socket.artifact_sha256)
    && socket.attribution?.basis === 'process_socket_before_after' && socket.attribution?.verdict === 'PASS'
    && hash(socket.attribution.artifact_sha256), 'native_discord_independent_socket_custody');
  const observed = reads.find(r => r.eg_receipt.replica === socket.replica), before = socket.process_before, after = socket.process_after;
  ctx.assert(observed && validEgReceipt(observed.eg_receipt) && sameProcess(before, after)
    && sameProcess(before, observed.eg_receipt.serving) && before.ready === true && after.ready === true
    && before.source === lease.discordTransport.source && before.image === lease.discordTransport.image,
  'native_discord_socket_actual_eg_process');
  const stamps = [before.observed_at, socket.started_at, socket.ended_at, after.observed_at].map(Date.parse);
  ctx.assert(stamps.every(Number.isFinite) && stamps.every((v, i) => !i || v >= stamps[i - 1]),
    'native_discord_socket_process_window');
  const health = observed.body.native_discord_activation.connection, at = Date.parse(observed.eg_receipt.ended_at);
  ctx.assert(health.connected === true && health.authenticated === true && health.ready === true
    && health.stale === false && health.lease === 'acquired' && health.identify === 'sent'
    && time(health.last_ready_at) && Date.parse(health.observed_at) <= at && Date.parse(health.valid_until) > at,
  'native_discord_selected_is_not_ready_without_actual_health');
  return socket;
}

export function assertDiscordGateway(ctx, lease, capture, reads, initial) {
  ctx.assert(capture?.schema === 'multi-eg-discord-gateway-v1' && hash(capture.artifact_sha256)
    && Array.isArray(capture.sessions) && capture.sessions.length === 1
    && same(capture.monitored_bot_ids, [lease.expected.botId, lease.expected.inactiveBotId])
    && capture.sessions_complete === true && time(capture.observed_from) && time(capture.observed_until),
  'native_discord_complete_isolated_gateway_capture');
  const socket = assertDiscordSocket(ctx, lease, capture.sessions[0], reads), calls = capture.discovery;
  ctx.assert(Date.parse(capture.observed_from) <= Date.parse(socket.started_at)
    && Date.parse(capture.observed_until) >= Date.parse(socket.ended_at), 'native_discord_session_inventory_window');
  if (initial) ctx.assert(socket.socket_id === initial.sessions[0].socket_id
    && socket.session_id === initial.sessions[0].session_id && capture.observed_from === initial.observed_from,
  'native_discord_no_unobserved_session_or_activation');
  ctx.assert(Array.isArray(calls) && calls.length === 2 && new Set(calls.map(c => c.request_id)).size === 2
    && calls.every(c => text(c.request_id) && c.method === 'GET' && c.status === 200
      && c.bot_id === lease.expected.botId && hash(c.artifact_sha256)), 'native_discord_actual_discovery_calls');
  const info = calls[1].response, limits = info?.session_start_limit;
  ctx.assert(calls[0].path === '/api/v10/users/@me' && calls[0].response?.id === lease.expected.botId
    && calls[0].response.bot === true && calls[1].path === '/api/v10/gateway/bot'
    && info?.url === 'wss://gateway.discord.gg/' && info.shards === 1
    && ['total', 'remaining', 'reset_after', 'max_concurrency'].every(k => Number.isSafeInteger(limits?.[k]) && limits[k] > 0)
    && limits.remaining <= limits.total, 'native_discord_bot_discovery_and_identify_budget');
  const f = capture.activation;
  ctx.assert(f?.socket_id === socket.socket_id && hash(f.artifact_sha256)
    && f.hello?.op === 10 && Number.isSafeInteger(f.hello.d?.heartbeat_interval) && f.hello.d.heartbeat_interval > 0
    && f.identify?.op === 2 && f.identify.d?.intents === discordIntents
    && f.identify.d.token_sha256 === lease.expected.credentialSha256
    && f.ready?.op === 0 && f.ready.t === 'READY' && Number.isSafeInteger(f.ready.s) && f.ready.s >= 0
    && f.ready.d?.user?.id === lease.expected.botId && f.ready.d.user.bot === true
    && f.ready.d.session_id === socket.session_id && f.heartbeat?.op === 1
    && Number.isSafeInteger(f.heartbeat.d) && f.heartbeat.d >= f.ready.s && f.heartbeatAck?.op === 11,
  'native_discord_captured_identify_ready_heartbeat');
  return socket;
}

export function discordReactionEventId(lease, session, sequence) {
  const h = createHash('sha256');
  for (const part of ['discord-reaction-dispatch-v1', lease.recordId, lease.expected.botId,
    lease.expected.sourceRevision, session, String(sequence)]) {
    const bytes = Buffer.from(part), length = Buffer.alloc(8); length.writeBigUInt64BE(BigInt(bytes.length)); h.update(length).update(bytes);
  }
  const bytes = h.digest().subarray(0, 16); bytes[6] = (bytes[6] & 15) | 64; bytes[8] = (bytes[8] & 63) | 128;
  const hex = bytes.toString('hex'), uuid = [hex.slice(0, 8), hex.slice(8, 12), hex.slice(12, 16), hex.slice(16, 20), hex.slice(20)].join('-');
  return 'discord:' + lease.expected.botId + ':reaction:' + uuid;
}

export function assertDiscordDispatch(ctx, socket, capture, input, sequence, checkpoint) {
  ctx.assert(capture?.schema === 'multi-eg-discord-dispatch-v1' && capture.socket_id === socket.socket_id
    && capture.session_sha256 === digest(socket.session_id) && hash(capture.artifact_sha256)
    && capture.frame?.op === 0 && capture.frame.t === input.name && capture.frame.s === sequence
    && same(capture.frame.d, input.data), 'native_discord_actual_exact_gateway_frame');
  // Gateway heartbeats report received, not committed, sequence. They prove
  // neither durable admission nor finality. Read the actual sealed journal too.
  ctx.assert(time(capture.sent_at) && time(capture.heartbeat?.observed_at)
    && Date.parse(capture.heartbeat.observed_at) >= Date.parse(capture.sent_at)
    && capture.heartbeat.socket_id === socket.socket_id && capture.heartbeat.frame?.op === 1
    && capture.heartbeat.frame.d === sequence && hash(capture.heartbeat.artifact_sha256),
  'native_discord_actual_received_sequence_heartbeat');
  ctx.assert(checkpoint?.schema === 'multi-eg-discord-checkpoint-v1' && checkpoint.seal_verified === true
    && hash(checkpoint.artifact_sha256) && checkpoint.record_id === socket.record_id
    && checkpoint.bot_id === socket.bot_id && checkpoint.source_revision === socket.source_revision
    && checkpoint.session_sha256 === digest(socket.session_id) && checkpoint.sequence === sequence
    && time(checkpoint.observed_at) && Date.parse(checkpoint.observed_at) >= Date.parse(capture.sent_at),
  'native_discord_actual_durable_journal_checkpoint');
}

export function assertDiscordAssociation(ctx, lease, capture, event, input, turn, original) {
  const e = lease.expected, reaction = input.name === 'MESSAGE_REACTION_ADD';
  ctx.assert(capture?.schema === 'multi-eg-native-event-v1' && text(event.eventRef)
    && capture.seal_verified === true && hash(capture.artifact_sha256) && capture.event_ref_sha256 === digest(event.eventRef)
    && turn.amRequests.some(r => r.request_id === capture.request_id)
    && ['eventId', 'conversationId', 'responseId', 'agentId', 'consumerId'].every(k => capture[k] === event[k])
    && same(capture.source, {type: 'discord', bot_id: e.botId})
    && same(capture.reply, {type: 'discord', bot_id: e.botId, channel_id: input.data.channel_id,
      message_id: reaction ? input.data.message_id : input.data.id, thread_id: null})
    && capture.actor_id === (reaction ? e.reactorId : e.senderId) && capture.actor_kind === 'human',
  'native_discord_actual_sealed_source_reply_and_actor');
  if (!reaction) ctx.assert(capture.text_sha256 === digest(input.data.content), 'native_discord_actual_message_content');
  else {
    const ce = capture.cloudevent;
    ctx.assert(ce?.specversion === '1.0' && ce.id === event.eventId && ce.type === 'cma.message.reaction.updated.v1'
      && ce.source === 'urn:cmaeg:discord:' + e.botId && ce.subject === input.data.message_id
      && ce.time === undefined && ce.datacontenttype === 'application/json'
      && ce.data?.message_id === input.data.message_id && ce.data.original_event_id === original.eventId
      && ce.data.target_kind === 'inbound' && ce.data.target_author?.id === e.senderId && ce.data.target_author.kind === 'human'
      && ce.data.operation === 'add' && ce.data.representation === 'delta'
      && same(ce.data.emoji, input.data.emoji) && ce.data.reaction_type === 0 && ce.data.burst === false
      && ce.data.old_reactions === undefined && ce.data.new_reactions === undefined
      && event.conversationId === original.conversationId && event.agentId === original.agentId,
    'native_discord_retained_reaction_delta_original_conversation');
  }
}

export function assertDiscordDelivery(ctx, lease, receipt, event, input) {
  delivered(ctx, event, receipt);
  const c = receipt.platformReceipt, message = input.data.message_id ?? input.data.id;
  ctx.assert(c?.schema === 'multi-eg-discord-delivery-v1' && text(c.request_id) && hash(c.artifact_sha256)
    && c.method === 'POST' && c.path === '/api/v10/channels/' + input.data.channel_id + '/messages' && c.status === 200
    && c.bot_id === lease.expected.botId && same(c.request, {content_sha256: event.expectedContentDigest,
      allowed_mentions: {parse: [], replied_user: false},
      message_reference: {message_id: message, channel_id: input.data.channel_id, fail_if_not_exists: false}})
    && c.response?.channel_id === input.data.channel_id && c.response.author?.id === lease.expected.botId
    && id(c.response.id), 'native_discord_actual_quoted_platform_answer');
}

export function assertDiscordActivity(ctx, lease, page, turns) {
  assertActivityPage(ctx, page, turns.map(t => t.event.eventId));
  for (const {event, input} of turns) {
    const r = page.items.find(v => v.event_id === event.eventId), c = r.consumers[0];
    ctx.assert(same(r.source, {type: 'discord', bot_id: lease.expected.botId}) && r.actor.kind === 'human'
      && r.actor.id === (input.name === 'MESSAGE_REACTION_ADD' ? lease.expected.reactorId : lease.expected.senderId)
      && r.conversation.id === input.data.channel_id && r.conversation.kind === (input.data.guild_id ? 'group' : 'private')
      && r.consumers.length === 1 && c.consumer_id === 'default' && c.agent_id === event.agentId
      && c.response_id === event.responseId && c.state === 'completed' && text(c.conversation_id)
      && c.deliveries.length === 1 && c.deliveries[0].step === 'answer' && c.deliveries[0].outcome === 'confirmed'
      && ['dispatched_at', 'accepted_at', 'produced_at', 'completed_at'].every(k => time(c[k]))
      && time(c.deliveries[0].confirmed_at), 'native_discord_actual_retained_completed_activity');
  }
}

export async function assertDiscordProducerUI(ctx, page, record) {
  const activation = page.getByRole('status', {name: 'Native Discord activation', exact: true});
  await visible(ctx, activation, 'native_discord_rendered_activation_status');
  ctx.assert(text(record.native_discord_activation.detail)
    && await activation.innerText() === record.native_discord_activation.detail, 'native_discord_rendered_actual_activation');
  const dialog = await openChannelNode(ctx, page, 'Producer: Discord (Direct Discord bot)');
  ctx.assert(await dialog.getByLabel('Discord bot user ID', {exact: true}).inputValue() === record.definition.sources[0].bot_id,
    'native_discord_rendered_bot_identity');
  for (const name of ['Receive server messages', 'Receive direct messages', 'Receive reactions to retained messages']) {
    const input = dialog.getByRole('checkbox', {name, exact: true});
    await visible(ctx, input, 'native_discord_rendered_enabled_intents');
    ctx.assert(await input.isChecked(), 'native_discord_saved_intents_visible');
  }
  await visible(ctx, dialog.getByText(/Message Content Intent is required\. Bot messages are not supported\./),
    'native_discord_bot_support_notice_required');
  await visible(ctx, dialog.getByText(/Reaction changes notify the original consumer/), 'native_discord_retained_reaction_notice');
  ctx.assert(await dialog.getByRole('button', {name: 'Messages from bots', exact: true}).count() === 0,
    'native_discord_no_unsupported_bot_control');
  await closeChannelInspector(ctx, page);
}

export const nativeDiscordCase = define(40, 'eg', ['browser', 'user-fixtures', 'eg_browser'], async ctx => {
  let lease, expected, initialGateway, socket, lastSequence;
  const initialRecords = new Map(), turns = [], dispatches = [];
  const bounded = (method, args = {}) => {
    ctx.assert(!ctx.signal?.aborted, 'native_discord_cancelled');
    return invoke(lease, method, {...args, signal: ctx.signal, timeoutMs: Math.max(1, Math.min(15000, ctx.remainingMs?.() ?? 15000))});
  };
  const read = async (replica, path) => {
    const r = await egRequest(ctx, lease, {path, replica, returnResponse: true, signal: ctx.signal});
    ctx.assert(validEgReceipt(r.eg_receipt) && r.eg_receipt.serving.source === lease.discordTransport.source
      && r.eg_receipt.serving.image === lease.discordTransport.image, 'native_discord_actual_http_attribution'); return r;
  };
  const inspect = async () => {
    const records = [];
    for (const replica of [ctx.am, peer(ctx.am)]) {
      const caps = await read(replica, '/v1/channel-capabilities');
      for (const inactive of [false, true]) {
        const channel = inactive ? expected.inactiveChannelId : lease.channelId;
        const r = await read(replica, '/v1/channels/' + segment(channel));
        assertDiscordSource(ctx, lease, r.body, caps.body, inactive);
        if (initialRecords.has(channel)) ctx.assert(same(saved(r.body), initialRecords.get(channel)), 'native_discord_unchanged_shared_source');
        else initialRecords.set(channel, saved(r.body));
        const activity = await read(replica, '/v1/channels/' + segment(channel) + '/activity');
        assertDiscordActivity(ctx, lease, activity.body, inactive ? [] : turns);
        if (!inactive) records.push(r);
      }
    }
    return records;
  };
  const send = async (input, original) => {
    const result = await bounded('gatewayDispatch', {socketId: socket.socket_id, ...input});
    ctx.assert(result.socket_id === socket.socket_id && Number.isSafeInteger(result.sequence)
      && result.sequence > lastSequence, 'native_discord_actual_new_gateway_sequence'); lastSequence = result.sequence;
    const wire = await bounded('gatewayCapture', {socketId: socket.socket_id, sequence: result.sequence});
    const checkpoint = await bounded('gatewayCheckpoint', {recordId: lease.recordId,
      sessionSha256: digest(socket.session_id), sequence: result.sequence});
    assertDiscordDispatch(ctx, socket, wire, input, result.sequence, checkpoint); dispatches.push({...wire, checkpoint});
    if (!input.data.author?.bot) {
      const eventId = input.name === 'MESSAGE_REACTION_ADD' ? discordReactionEventId(lease, socket.session_id, result.sequence)
        : 'discord:' + expected.botId + ':' + input.data.id;
      const turn = await bounded('waitForTurn', {eventId});
      const event = {...turn, eventId, platform: 'discord', trustMode: 'native', destination: input.data.channel_id,
        expectedSender: expected.botId, expectedContentDigest: digest(input.completionText)};
      ctx.assert(turn.eventId === eventId && turn.consumerId === expected.consumerId, 'native_discord_exact_native_turn');
      await retainTurn(ctx, event, turn);
      const capture = await bounded('nativeEventCapture', {eventId});
      assertDiscordAssociation(ctx, lease, capture, event, input, turn, original);
      const context = await bounded('providerContext', {eventId});
      for (const value of input.name === 'MESSAGE_REACTION_ADD'
        ? [original.eventId, 'cma.message.reaction.updated.v1', input.data.emoji.name] : [input.data.content])
        assertJourneyContext(ctx, event, context, value, []);
      const receipt = await bounded('waitForDelivery', {eventId}); assertDiscordDelivery(ctx, lease, receipt, event, input);
      turns.push({event, input, turn, receipt});
      await ctx.observe('native_discord_turn', async () => ({event_sha256: digest(eventId),
        provider_sha256: digest(capture), model_sha256: digest(context), delivery_sha256: digest(receipt)}));
    }
    await ctx.observe('native_discord_gateway_dispatch', async () => ({schema: 'multi-eg-discord-dispatch-proof-v1',
      socket_sha256: digest(socket.socket_id), sequence: result.sequence, artifact_sha256: wire.artifact_sha256,
      heartbeat_sha256: wire.heartbeat.artifact_sha256, checkpoint_sha256: checkpoint.artifact_sha256}));
    assertRelayCounters(ctx, await bounded('drain', {eventIds: turns.map(t => t.event.eventId)}), turns.length, turns.length);
  };
  await ctx.step('step_1', async () => {
    lease = await acquire(ctx, 'native_discord'); expected = validateNativeDiscordFixture(lease);
    const records = await inspect(); initialGateway = await bounded('gatewaySnapshot');
    socket = assertDiscordGateway(ctx, lease, initialGateway, records); lastSequence = initialGateway.activation.heartbeat.d;
    assertRelayCounters(ctx, await bounded('counters'), 0, 0);
    const session = await openBrowser(ctx, 'user', {channelEditor: true});
    for (const method of ['egEvidence', 'switchEG', 'networkIdle'])
      if (typeof session[method] !== 'function') throw new Blocked('native_discord_actual_eg_browser_required');
    const guard = await guardDiscoveryWrites(ctx, session.page);
    for (const replica of [ctx.am, peer(ctx.am)]) {
      await session.switchEG(replica);
      const selected = await openListedChannel(ctx, session, records[0].body, () => goto(session.page, '/event-gateway'));
      ctx.assert(selected.receipt.replica === replica && same(saved(selected.body), initialRecords.get(lease.channelId))
        && selected.receipt.serving.source === lease.discordTransport.source
        && selected.receipt.serving.image === lease.discordTransport.image, 'native_discord_browser_actual_saved_source');
      await assertDiscordProducerUI(ctx, session.page, selected.body);
    }
    await uiReceipt(ctx, session.page, 'native_discord_producer_ui');
    const browser = await invoke(ctx.fixture('browser'), 'evidence', session.page);
    ctx.assert(browser.compiled_source === lease.discordTransport.source && guard.attempts() === 0,
      'native_discord_exact_compiled_ui_read_only');
    await ctx.observe('native_discord_activation', async () => ({schema: 'multi-eg-discord-activation-proof-v1',
      artifact_sha256: initialGateway.artifact_sha256, socket_sha256: digest(socket.socket_id),
      process_sha256: digest(socket.process_before), binding_sha256: lease.discordTransport.artifact_sha256}));
  });
  await ctx.step('step_2', async () => {
    // Fresh snowflakes obey the native journal's seven-day dedup and clock checks.
    const message = (group, bot = false) => ({name: 'MESSAGE_CREATE', completionText: 'Discord answer ' + randomUUID(), data: {
      id: ((BigInt(Date.now() - 2000 - 1420070400000) << 22n) + BigInt(randomInt(1, 2 ** 22))).toString(),
      channel_id: group ? expected.serverChat : expected.privateChat, ...(group ? {guild_id: expected.guildId} : {}),
      author: {id: bot ? expected.botSenderId : expected.senderId, username: 'synthetic', bot, system: false},
      content: (group ? '<@' + expected.botId + '> ' : '') + 'Native Discord message ' + randomUUID(),
      timestamp: new Date(Date.now() - 2000).toISOString(), type: 0, attachments: [],
      mentions: group ? [{id: expected.botId}] : []}});
    await send(message(false)); await send(message(true));
    const original = turns[1].event;
    await send({name: 'MESSAGE_REACTION_ADD', completionText: 'Discord reaction answer ' + randomUUID(), data: {
      user_id: expected.reactorId, channel_id: expected.serverChat, guild_id: expected.guildId,
      message_id: turns[1].input.data.id, message_author_id: expected.senderId,
      member: {user: {id: expected.reactorId, username: 'synthetic-reactor', bot: false, system: false}},
      emoji: {id: null, name: '👍'}, type: 0, burst: false}}, original);
    await send(message(true, true));
  });
  await ctx.step('expected_result', async () => {
    ctx.assert(turns.length === 3 && dispatches.length === 4
      && turns[0].event.conversationId !== turns[1].event.conversationId
      && new Set(turns.map(t => t.event.responseId)).size === 3, 'native_discord_exact_dm_server_reaction_turns');
    const effects = await bounded('captureEffects');
    ctx.assert(Array.isArray(effects) && effects.length === 3 && turns.every(({event, receipt}) =>
      effects.filter(e => e.eventId === event.eventId && e.responseId === event.responseId && e.step === 'answer'
        && e.attempts === 1 && e.platformRequestId === receipt.platformReceipt.request_id).length === 1),
    'native_discord_exact_answer_effects_no_bot_or_extra_ack');
    const records = await inspect(), final = await bounded('gatewaySnapshot');
    assertDiscordGateway(ctx, lease, final, records, initialGateway);
    // The shared durable inbox may be dispatched by the socket owner's peer.
    ctx.assert(records.some(r => r.body.native_discord_activation.connection.last_dispatch_outcome === 'refused'
      && Date.parse(r.body.native_discord_activation.connection.last_dispatch_at) >= Date.parse(dispatches.at(-1).sent_at)),
    'native_discord_bot_frame_durably_refused');
    ctx.assert(Date.parse(final.observed_until) >= Date.parse(dispatches.at(-1).checkpoint.observed_at),
      'native_discord_inactive_source_excluded_through_finality');
    assertRelayCounters(ctx, await bounded('drain', {eventIds: turns.map(t => t.event.eventId)}), 3, 3);
    await ctx.observe('native_discord_finality', async () => ({artifact_sha256: final.artifact_sha256,
      effects_sha256: digest(effects), turns: 3, deliveries: 3, active_sockets: final.sessions.length}));
  });
});
