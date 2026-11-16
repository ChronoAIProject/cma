import {randomUUID} from 'node:crypto';
import {Blocked} from '../../runner.mjs';
import {validEgReceipt} from '../../evidence-scope.mjs';
import {define, acquire, openBrowser, invoke, visible, goto, peer, same, digest, segment,
  egRequest, retainTurn, uiReceipt} from './helpers.mjs';
import {channelReceipt, openListedChannel} from './channel-browser.mjs';

const markerPattern = /EG024_[a-f0-9]{32}/g;
const marker = () => 'EG024_' + randomUUID().replaceAll('-', '');
const uuid = /^[a-f0-9]{8}-[a-f0-9]{4}-4[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}$/;
const pick = async (page, label, name) => {
  await page.getByRole('button', {name: label, exact: true}).click();
  await page.getByRole('menuitemradio', {name, exact: true}).click();
};
const without = (value, keys) => Object.fromEntries(Object.entries(value).filter(([key]) => !keys.includes(key)));

// Match the real editor's credential-free update projection. Server identity,
// routing and secret hashes are checked separately, never submitted as a draft.
export function editableChannel(record) {
  const d = record.definition;
  return {...without(d, ['record_id', 'creation', 'owner', 'profile_digest', 'secrets', 'created_at',
    'version', 'routing', 'durable_custom_execution']),
  provider: {...without(d.provider, ['binding_id', 'level']), slug: record.provider.slug},
  sources: d.sources.map(source => without(source, ['jwks_url']))};
}

export function validateAdmissionFixture(lease, record) {
  const ids = lease.identities, expected = lease.expected, d = record?.definition;
  if (!ids || !Array.isArray(ids.senders) || ids.senders.length !== 2
    || !Array.isArray(ids.chats) || ids.chats.length !== 2
    || ![...ids.senders, ids.deniedSender, ids.botSender, ...ids.chats, ids.deniedChat].every(v => typeof v === 'string' && v.length > 0)
    || new Set([...ids.senders, ids.deniedSender, ids.botSender]).size !== 4 || new Set([...ids.chats, ids.deniedChat]).size !== 3
    || !expected?.replySender || !/^[a-f0-9]{64}$/.test(expected.ackContentDigest ?? '')
    || record?.record_id !== lease.recordId || record?.channel_id !== lease.channelId
    || !d?.name || d.sources?.length !== 1 || d.sources[0].type !== 'nyxid_relay'
    || d.sources[0].platform !== 'telegram' || !d.sources[0].bot_id || !d.sources[0].bot_username
    || d.sources[0].trust_normalized === true || d.sources[0].admission?.type !== 'all'
    || record.provider?.slug !== 'cma_codex' || d.provider?.target?.type === 'existing_agent'
    || d.profile?.environment?.session_affinity === 'source_user' || d.routing !== undefined
    || d.connection_ref !== undefined || !same(d.partition, {version: 1, key: 'conversation_and_sender'})
    || !same(d.reply, {auto_final: true, ack_after_ms: 0, sinks: [{type: 'source'}]}))
    throw new Blocked('channel_admission_isolated_verified_telegram_fixture_required');
  return ids;
}

function serializedDraft(draft) {
  const value = structuredClone(draft);
  // Rust omits the default bot_messages=deny; no other mismatched field is erased.
  for (const source of value.sources) if (source.admission?.bot_messages === 'deny') delete source.admission.bot_messages;
  return value;
}

export function assertChannelUpdate(ctx, before, after, draft) {
  ctx.assert(after?.record_id === before.record_id && after.channel_id === before.channel_id
    && after.version === before.version + 1, 'admission_update_identity_revision');
  ctx.assert(same(editableChannel(after), serializedDraft(draft)), 'admission_update_exact_definition');
  for (const key of ['owner', 'secrets', 'profile_digest', 'routing', 'creation'])
    ctx.assert(same(after.definition[key], before.definition[key]), 'admission_update_preserves_' + key);
  ctx.assert(after.provider.binding_id === before.provider.binding_id, 'admission_update_preserves_binding');
}

// Keep one guard installed for the whole case. It validates before forwarding,
// never retries a PUT, and preserves any duplicate/unexpected mutation failure.
export async function guardChannelUpdates(ctx, session) {
  const page = session.page, pending = new Set(), failures = [], keys = new Set(); let active;
  const match = url => url.origin === session.origin && url.pathname.startsWith('/api/event-gateway/');
  const dispatch = async route => {
    const req = route.request();
    if (['GET', 'HEAD', 'OPTIONS'].includes(req.method())) return route.fallback();
    const task = active; let fulfilled = false, result; const errors = [];
    try {
      ctx.assert(task && ++task.attempts === 1 && req.method() === 'PUT'
        && new URL(req.url()).pathname === '/api/event-gateway/v1/channels/' + segment(task.before.channel_id),
      'admission_unexpected_browser_mutation');
      const body = req.postDataJSON(), key = (await req.allHeaders())['idempotency-key'];
      ctx.assert(same(body, {...task.draft, expected_version: task.before.version})
        && uuid.test(key ?? '') && !keys.has(key), 'admission_exact_ui_update_intent');
      keys.add(key);
      ctx.assert(!ctx.signal?.aborted, 'admission_update_cancelled');
      const response = await route.fetch({maxRetries: 0, maxRedirects: 0,
        timeout: Math.max(1, ctx.remainingMs?.() ?? 30000)});
      try {
        ctx.assert(response.status() === 200, 'admission_update_http_status');
        const record = await response.json(), requestId = response.headers()['x-request-id'];
        assertChannelUpdate(ctx, task.before, record, task.draft);
        ctx.assert(typeof requestId === 'string' && requestId.length > 0, 'admission_update_request_id');
        result = {record, requestId};
        await route.fulfill({response}); fulfilled = true;
      } catch (error) {errors.push(error);}
      try {await response.dispose();} catch (error) {errors.push(error);}
    } catch (error) {errors.push(error);}
    if (!fulfilled) try {await route.abort('failed');} catch (error) {errors.push(error);}
    if (errors.length) {
      const error = new AggregateError(errors, 'admission_browser_mutation_failed');
      failures.push(error); result = {error};
    }
    task?.resolve(result);
  };
  const handler = route => {
    const work = dispatch(route); pending.add(work); return work.finally(() => pending.delete(work));
  };
  const cancel = () => active?.resolve({error: Error('admission_update_cancelled')});
  ctx.signal?.addEventListener('abort', cancel);
  await page.route(match, handler);
  ctx.defer('remove_admission_update_guard', async () => {
    ctx.signal?.removeEventListener('abort', cancel);
    const errors = [];
    try {await page.unroute(match, handler);} catch (error) {errors.push(error);}
    const settled = await Promise.allSettled([...pending]);
    errors.push(...settled.filter(v => v.status === 'rejected').map(v => v.reason), ...failures);
    if (errors.length) throw new AggregateError(errors, 'admission_update_cleanup_failed');
  });
  return {async save(before, draft, action) {
    ctx.assert(!active && !ctx.signal?.aborted, 'admission_update_serialized');
    let resolve; const completed = new Promise(done => {resolve = done;});
    active = {before, draft, resolve, attempts: 0};
    try {
      const [, result] = await Promise.all([action(), completed]);
      if (result.error) throw result.error;
      await invoke(session, 'networkIdle');
      if (failures.length) throw failures[0];
      const receipt = channelReceipt(ctx, await invoke(session, 'egEvidence'), result.requestId, 'PUT');
      await ctx.observe('admission_ui_update', async () => ({schema: 'multi-eg-request-v1', receipt}));
      return {...result, receipt};
    } finally {active = undefined;}
  }};
}

export async function fillAdmission(page, ids, scoped) {
  await pick(page, 'Admission policy', scoped ? 'Sender, chat and group rules' : 'Allowed senders');
  const add = async (label, values) => {
    for (let i = 0; i < values.length; i++) {
      await page.getByRole('button', {name: 'Add ' + label, exact: true}).click();
      await page.getByLabel(`${label} ${i + 1}`, {exact: true}).fill(values[i]);
    }
  };
  if (!scoped) {await add('Allowed sender IDs', ids.senders); return {type: 'allowlist', sender_ids: ids.senders};}
  await pick(page, 'Senders policy', 'allowlist'); await add('Senders IDs', ids.senders);
  await pick(page, 'Chats policy', 'allowlist'); await add('Chats IDs', ids.chats);
  await pick(page, 'Group messages', 'mention'); await pick(page, 'Messages from bots', 'Ignore bots');
  return {type: 'scoped', senders: {type: 'allowlist', ids: ids.senders},
    chats: {type: 'allowlist', ids: ids.chats}, groups: 'mention', bot_messages: 'deny'};
}

export async function fillReplyPolicy(page, grouping, autoFinal, ackAfterMs) {
  await pick(page, 'Group incoming messages by', grouping === 'conversation'
    ? 'Each conversation' : 'Each sender in each conversation');
  await page.getByRole('checkbox', {name: 'Send final answers automatically', exact: true}).setChecked(autoFinal);
  await page.getByLabel('Acknowledgement delay (ms)', {exact: true}).fill(String(ackAfterMs));
  await pick(page, 'Destination 1 type', 'Original conversation');
}

export function assertIngress(ctx, result, input, reason) {
  ctx.assert(typeof result?.eventId === 'string' && result.eventId.length > 0
    && result.ingress?.status === 202 && validEgReceipt(result.ingress.receipt)
    && result.ingress.receipt.status === 202 && result.ingress.receipt.method === 'POST'
    && result.ingress.receipt.replica === input.replica, 'admission_actual_ingress');
  const body = result.ingress.body;
  if (reason) ctx.assert(same(body, {status: 'refused', reason}), 'admission_exact_refusal');
  else ctx.assert(body?.status === 'accepted' && body.event_id === result.eventId
    && body.conversation_id === result.conversationId, 'admission_exact_acceptance');
  ctx.assert(result.wire?.senderId === input.senderId && result.wire.chatId === input.chatId
    && result.wire.isGroup === true && result.wire.mentioned === input.mentioned
    && result.wire.isBotSender === input.isBotSender && result.wire.textDigest === digest(input.text),
  'admission_actual_signed_wire_facts');
}

export function assertProviderContext(ctx, event, context, expectedMarkers) {
  ctx.assert(context?.eventId === event.eventId && context.responseId === event.responseId
    && context.agentId === event.agentId && context.conversationId === event.conversationId
    && context.requestCount === 1 && Array.isArray(context.messages), 'admission_independent_provider_context');
  const texts = context.messages.flatMap(message => {
    ctx.assert(['system', 'developer', 'user', 'assistant'].includes(message.role)
      && (typeof message.content === 'string' || Array.isArray(message.content)
        && message.content.every(part => ['input_text', 'output_text', 'text'].includes(part.type)
          && typeof part.text === 'string')), 'admission_provider_text_history_required');
    return typeof message.content === 'string' ? [message.content] : message.content.map(part => part.text);
  });
  const seen = [...new Set(texts.flatMap(text => [...text.matchAll(markerPattern)].map(match => match[0])))].sort();
  ctx.assert(same(seen, [...new Set(expectedMarkers)].sort()), 'admission_exact_context_history');
}

export function assertPartitionMapping(ctx, record, event, partition) {
  ctx.assert(partition?.consumer_id === event.consumerId && partition.transport_conversation_id === event.conversationId
    && partition.mapping?.binding_id === record.provider.binding_id
    && partition.mapping.conversation_id === event.conversationId && partition.mapping.agent_id === event.agentId
    && Number.isSafeInteger(partition.mapping.generation) && partition.mapping.generation > 0,
  'admission_actual_partition_agent_mapping');
}

export function assertReplyEffects(ctx, event, effects, expected) {
  ctx.assert(Array.isArray(effects) && effects.length === expected.length, 'admission_exact_reply_count');
  const ids = new Set();
  for (let i = 0; i < effects.length; i++) {
    const effect = effects[i], want = expected[i];
    ctx.assert(effect.eventId === event.eventId && effect.responseId === event.responseId
      && effect.destination === event.destination && effect.sender === event.expectedSender
      && effect.platform === 'telegram' && effect.sink === 'source'
      && effect.step === want.step && effect.contentDigest === want.contentDigest
      && effect.status === 'delivered' && effect.attempts === 1
      && typeof effect.deliveryId === 'string' && effect.deliveryId.length > 0 && !ids.has(effect.deliveryId),
    'admission_exact_reply_destination_content');
    ids.add(effect.deliveryId);
    if (want.step === 'ack') ctx.assert(Number.isFinite(effect.elapsedSinceAcceptedMs)
      && effect.elapsedSinceAcceptedMs >= want.ackAfterMs, 'admission_ack_not_early');
  }
}

export function assertAdmissionCounters(ctx, counts, turns, deliveries) {
  ctx.assert(counts?.turns === turns && counts.effects === turns && counts.deliveries === deliveries
    && counts.pending === 0 && counts.unknown === 0 && counts.lateEffects === 0, 'admission_settled_exact_counts');
}

export const channelAdmissionCase = define(24, 'browser', ['browser', 'user-fixtures', 'eg_browser'], async ctx => {
  let lease, session, guard, current, ids; const accepted = [], rejected = [], contexts = new Map();
  let turns = 0, deliveries = 0;
  const reopen = async saved => {
    await session.switchEG(peer(saved.receipt.replica));
    const selected = await openListedChannel(ctx, session, saved.record, () => goto(session.page, '/event-gateway'));
    ctx.assert(selected.receipt.replica !== saved.receipt.replica
      && selected.receipt.serving.instance_id !== saved.receipt.serving.instance_id
      && same(selected.body.definition, saved.record.definition), 'admission_peer_saved_definition');
    current = selected.body;
  };
  const producer = async scoped => {
    const page = session.page;
    await page.getByRole('button', {name: 'Producer: Telegram (NyxID relay)', exact: true}).click();
    const dialog = page.getByRole('dialog', {name: 'Telegram', exact: true});
    await visible(ctx, dialog, 'admission_producer_inspector');
    const draft = editableChannel(current);
    draft.sources[0].admission = await fillAdmission(page, ids, scoped);
    await reopen(await guard.save(current, draft, () => dialog.getByRole('button', {name: 'Save producer', exact: true}).click()));
  };
  const policy = async (grouping, autoFinal, ackAfterMs) => {
    const page = session.page;
    await page.getByRole('button', {name: `Channel: ${current.definition.name} (Route events to agents)`, exact: true}).click();
    const dialog = page.getByRole('dialog', {name: 'Channel settings', exact: true});
    await visible(ctx, dialog, 'admission_channel_inspector');
    await fillReplyPolicy(page, grouping, autoFinal, ackAfterMs);
    const draft = editableChannel(current);
    draft.partition = {...draft.partition, key: grouping};
    draft.reply = {...draft.reply, auto_final: autoFinal, ack_after_ms: ackAfterMs, sinks: [{type: 'source'}]};
    await reopen(await guard.save(current, draft, () => dialog.getByRole('button', {name: 'Save channel', exact: true}).click()));
  };
  const send = async (senderId, chatId, {reason, mentioned = true, isBotSender = false} = {}) => {
    const text = marker(), completionText = 'Synthetic admission reply ' + randomUUID();
    const input = {platform: 'telegram', trustMode: 'nyxid_relay', senderId, chatId, isGroup: true,
      mentioned, isBotSender, text, completionText, holdCompletion: !reason,
      replica: (accepted.length + rejected.length) % 2 ? peer(ctx.am) : ctx.am};
    const event = await invoke(lease, 'platformSend', input);
    assertIngress(ctx, event, input, reason);
    await ctx.observe('admission_ingress', async () => ({schema: 'multi-eg-request-v1', receipt: event.ingress.receipt}));
    if (reason) {
      rejected.push(event.eventId);
      assertAdmissionCounters(ctx, await invoke(lease, 'quiesce', {eventId: event.eventId, timeoutMs: 5000}), turns, deliveries);
      return;
    }
    const turn = await invoke(lease, 'waitForTurn', {eventId: event.eventId, stage: 'accepted', timeoutMs: 15000});
    await retainTurn(ctx, event, turn);
    ctx.assert(event.destination === chatId && event.expectedSender === lease.expected.replySender,
      'admission_original_conversation_destination');
    const group = current.definition.partition.key, reply = current.definition.reply;
    const key = JSON.stringify(group === 'conversation' ? [group, chatId] : [group, chatId, senderId]);
    const prior = contexts.get(key), history = [...(prior?.history ?? []), text];
    if (prior) ctx.assert(event.agentId === prior.agentId && event.conversationId === prior.conversationId,
      'admission_shared_agent_context');
    else ctx.assert([...contexts.values()].every(other => other.conversationId !== event.conversationId),
      'admission_isolated_conversation_context');
    const context = await invoke(lease, 'providerContext', {eventId: event.eventId});
    assertProviderContext(ctx, event, context, history);
    contexts.set(key, {agentId: event.agentId, conversationId: event.conversationId, history});
    const expected = [];
    if (reply.ack_after_ms > 0) {
      expected.push({step: 'ack', contentDigest: lease.expected.ackContentDigest, ackAfterMs: reply.ack_after_ms});
      await invoke(lease, 'waitForDelivery', {eventId: event.eventId, step: 'ack', timeoutMs: 5000});
      assertReplyEffects(ctx, event, await invoke(lease, 'captureEffects', {eventId: event.eventId}), expected);
    }
    // The fixture releases only the held provider completion. It must never
    // synthesize an EG reply or another turn, and cleanup releases abandoned holds.
    await invoke(lease, 'releaseCompletion', {eventId: event.eventId});
    if (reply.auto_final) expected.push({step: 'answer', contentDigest: digest(completionText)});
    turns++; deliveries += expected.length;
    assertAdmissionCounters(ctx, await invoke(lease, 'drain', {eventIds: [event.eventId], timeoutMs: 15000}), turns, deliveries);
    assertReplyEffects(ctx, event, await invoke(lease, 'captureEffects', {eventId: event.eventId}), expected);
    // The real inspection endpoint takes the serialized source partition key;
    // it returns one transport/provider mapping, not grouping/conversations[].
    const partitionKey = group === 'conversation' ? chatId : chatId + '\u001f' + senderId;
    const partition = await egRequest(ctx, lease, {path: '/v1/channels/' + segment(lease.channelId)
      + '/partition?partition_key=' + segment(partitionKey), replica: peer(event.ingress.receipt.replica)});
    assertPartitionMapping(ctx, current, event, partition);
    accepted.push({event, expected});
  };
  await ctx.step('step_1', async () => {
    lease = await acquire(ctx, 'channel_admission');
    current = await egRequest(ctx, lease, {path: '/v1/channels/' + segment(lease.channelId)});
    ids = validateAdmissionFixture(lease, current);
    assertAdmissionCounters(ctx, await invoke(lease, 'counters'), 0, 0);
    session = await openBrowser(ctx, 'user', {channelAdmission: true});
    for (const method of ['egEvidence', 'switchEG', 'networkIdle'])
      if (typeof session[method] !== 'function') throw new Blocked('channel_admission_actual_eg_browser_required');
    await openListedChannel(ctx, session, current, () => goto(session.page, '/event-gateway'));
    guard = await guardChannelUpdates(ctx, session);
    await producer(false);
    await send(ids.senders[0], ids.chats[0]);
    await send(ids.deniedSender, ids.chats[0], {reason: 'sender_denied'});
    await producer(true);
    await send(ids.deniedSender, ids.chats[0], {reason: 'sender_denied'});
    await send(ids.senders[0], ids.deniedChat, {reason: 'chat_denied'});
    await send(ids.senders[0], ids.chats[0], {reason: 'group_not_addressed', mentioned: false});
    await send(ids.botSender, ids.chats[0], {reason: 'bot_sender', isBotSender: true});
  });
  await ctx.step('step_2', async () => {
    for (const grouping of ['conversation_and_sender', 'conversation']) {
      await policy(grouping, true, grouping === 'conversation' ? 100 : 0);
      for (const [sender, chat] of [[0, 0], [1, 0], [0, 1], [0, 0]]) await send(ids.senders[sender], ids.chats[chat]);
    }
    await policy('conversation', false, 0);
    await send(ids.senders[1], ids.chats[1]);
    ctx.assert(accepted.length === 10 && rejected.length === 5 && contexts.size === 5,
      'admission_grouping_matrix_complete');
  });
  await ctx.step('expected_result', async () => {
    const final = await invoke(lease, 'drain', {timeoutMs: 15000});
    assertAdmissionCounters(ctx, final, 10, 13);
    for (const {event, expected} of accepted)
      assertReplyEffects(ctx, event, await invoke(lease, 'captureEffects', {eventId: event.eventId}), expected);
    for (const eventId of rejected) ctx.assert((await invoke(lease, 'captureEffects', {eventId})).length === 0,
      'admission_refused_never_delivered');
    ctx.assert(new Set([...accepted.map(({event}) => event.eventId), ...rejected]).size === 15,
      'admission_unique_event_intents');
    await ctx.observe('admission_grouping_reply_totals', async () => ({accepted: accepted.length,
      refused: rejected.length, contexts: contexts.size, turns: final.turns, deliveries: final.deliveries,
      pending: final.pending, unknown: final.unknown}));
    await uiReceipt(ctx, session.page);
  });
});
