import {caseOf, check, equal, key, otherAM, readOtherAM, request, segment, take} from './common.mjs';

// Decode bounded, completed raw SSE captured by the real shared transport.
// An application JSON object or fabricated event summary is not a stream.
export function sseFrames(reply) {
  check(reply.headers?.['content-type']?.split(';')[0].trim() === 'text/event-stream', 'actual_sse_content_type_required');
  check(typeof reply.body === 'string' && Buffer.byteLength(reply.body) <= 4 * 1024 * 1024, 'bounded_raw_sse_required');
  const frames = [];
  let data = [], event, id;
  const lines = reply.body.replace(/^\uFEFF/, '').split(/\r\n|\n|\r/);
  for (const line of lines) {
    if (line === '') {
      if (data.length) {
        const raw = data.join('\n');
        check(Buffer.byteLength(raw) <= 2 * 1024 * 1024, 'sse_frame_bound');
        let body;
        try { body = JSON.parse(raw); } catch { throw Error('sse_invalid_json'); }
        frames.push({event, id, body});
      }
      data = []; event = undefined; id = undefined;
      continue;
    }
    if (line.startsWith(':')) continue;
    const separator = line.indexOf(':');
    const name = separator < 0 ? line : line.slice(0, separator);
    const value = separator < 0 ? '' : line.slice(separator + 1).replace(/^ /, '');
    if (name === 'data') data.push(value);
    else if (name === 'event') event = value;
    else if (name === 'id') { check(!value.includes('\0'), 'sse_invalid_id'); id = value; }
  }
  check(data.length === 0 && frames.length > 0, 'sse_truncated_or_empty');
  return frames;
}

export function assertResponses(ctx, reply, agentId, expectedText, expectedStatus = 'completed') {
  const frames = sseFrames(reply);
  let responseId, terminal;
  for (const [index, frame] of frames.entries()) {
    const event = frame.body;
    ctx.assert(frame.id === undefined && (frame.event === undefined || frame.event === event.type), 'responses_sse_envelope');
    ctx.assert(event.sequence_number === index && terminal === undefined, 'responses_sequence_order');
    if (event.response) {
      const response = event.response;
      ctx.assert(response.id.startsWith('resp_' + agentId + '_') && /^\d+$/.test(response.id.slice(('resp_' + agentId + '_').length)),
        'responses_exact_agent_identity');
      responseId ??= response.id;
      ctx.assert(response.id === responseId, 'responses_single_response_identity');
      const status = event.type === 'response.created' ? 'in_progress' : event.type.slice('response.'.length);
      ctx.assert(response.status === status, 'responses_lifecycle_consistent');
      if (['completed','failed','incomplete'].includes(response.status)) terminal = response;
    }
    ctx.assert(typeof responseId === 'string' && (event.response_id === undefined || event.response_id === responseId),
      'responses_frame_identity');
  }
  ctx.assert(frames[0].body.type === 'response.created' && terminal?.status === expectedStatus, 'responses_terminal_required');
  if (expectedStatus === 'completed') {
    const messages = terminal.output.filter(item => item.type === 'message');
    ctx.assert(messages.length === 1 && messages[0].role === 'assistant' && messages[0].status === 'completed'
      && messages[0].content.map(part => part.text).join('') === expectedText, 'responses_single_expected_output');
  } else ctx.assert(terminal.incomplete_details?.reason === 'interrupted', 'responses_interrupt_settled');
  return terminal;
}

export function assertAgui(ctx, reply, agentId, expectedText, expectedRun) {
  const frames = sseFrames(reply), messages = new Map(), completed = new Set();
  let lastCursor = 0;
  for (const frame of frames) {
    const event = frame.body;
    ctx.assert(event.threadId === agentId && typeof event.runId === 'string' && event.runId.length > 0, 'agui_thread_run_identity');
    ctx.assert(frame.event === undefined || frame.event === 'message' || frame.event === event.type, 'agui_sse_envelope');
    if (frame.id !== undefined) {
      const cursor = Number(frame.id);
      ctx.assert(event['cma:live'] !== true && /^[1-9]\d*$/.test(frame.id)
        && Number.isSafeInteger(cursor) && cursor > lastCursor, 'agui_durable_cursor_order');
      lastCursor = cursor;
    }
    if (event['cma:live'] === true) continue;
    if (event.type === 'TEXT_MESSAGE_START') {
      ctx.assert(!messages.has(event.messageId), 'agui_no_duplicate_message');
      messages.set(event.messageId, {role: event.role, text: '', ended: false, runId: event.runId});
    } else if (event.type === 'TEXT_MESSAGE_CONTENT' || event.type === 'TEXT_MESSAGE_END') {
      const message = messages.get(event.messageId);
      ctx.assert(message && !message.ended && message.runId === event.runId, 'agui_message_frame_order');
      if (event.type === 'TEXT_MESSAGE_CONTENT') message.text += event.delta;
      else message.ended = true;
    } else if (event.type === 'RUN_FINISHED' && event.outcome?.type === 'complete') completed.add(event.runId);
    else if (event.type === 'RUN_ERROR') ctx.assert(false, 'agui_run_error');
  }
  const run = expectedRun ?? [...completed].at(-1);
  ctx.assert(typeof run === 'string' && completed.has(run), 'agui_run_completed');
  const outputs = [...messages.values()].filter(message => message.runId === run && message.role === 'assistant');
  ctx.assert(outputs.length === 1 && outputs[0].ended && outputs[0].text === expectedText, 'agui_single_expected_output');
  return {runId: run, cursor: lastCursor};
}

async function conversationItems(ctx, agentId, version, am = ctx.am) {
  const all = [], seen = new Set();
  let after;
  for (let page = 0; page < 50; page++) {
    const path = '/api/' + version + '/conversations/' + segment(agentId) + '/items?limit=20'
      + (version === 'v2' ? '&order=asc' : '') + (after ? '&after=' + segment(after) : '');
    const reply = await request(ctx, 'GET', path, undefined, {agentId, am});
    const body = reply.body;
    ctx.assert(body.object === 'list' && Array.isArray(body.data) && body.data.length <= 20
      && body.first_id === (body.data[0]?.id ?? null) && body.last_id === (body.data.at(-1)?.id ?? null), 'conversation_item_page_cursors');
    for (const item of body.data) {
      ctx.assert(typeof item.id === 'string' && !seen.has(item.id), 'conversation_no_duplicate_items');
      seen.add(item.id); all.push(item);
    }
    if (body.has_more === false) return all;
    ctx.assert(body.data.length > 0 && body.last_id !== after, 'conversation_item_progress');
    after = body.last_id;
  }
  throw Error('conversation_item_page_budget');
}

const numbers = [24,25,26,27,28,30,31,32,49,50,51,52,84,85];
export const compatibilityCases = numbers.map(number => caseOf(number, 'agent', ['api-fixtures'], async ctx => {
  let fixture, before, completed, cancelled, agui, responseIntent, aguiIntent, responseReceipt, created = [];
  const agentPath = suffix => '/api/v1/agents/' + segment(fixture.agentId) + suffix;
  const opts = () => ({agentId: fixture.agentId, timeout: 120000});
  async function newConversation() {
    let conversation, creationReceipt, dispatched = false;
    ctx.defer('cleanup_new_conversation', async () => {
      if (!dispatched) return;
      check(conversation?.id, 'conversation_create_custody_unknown');
      await fixture.cleanupConversation(ctx, conversation.id);
    });
    try {
      dispatched = true;
      creationReceipt = await request(ctx, 'POST', '/api/v1/conversations', fixture.conversationRequest);
      conversation = creationReceipt.body;
    } catch (error) { conversation = error.response?.body; throw error; }
    ctx.assert(conversation.object === 'conversation' && typeof conversation.id === 'string'
      && conversation.metadata.workspace === fixture.conversationRequest.metadata.workspace
      && conversation.metadata.sandbox === fixture.conversationRequest.metadata.sandbox, 'conversation_creation_exact_parent');
    ctx.assert(conversation.id !== fixture.agentId && !created.some(value => value.id === conversation.id), 'conversation_new_child_identity');
    created.push(conversation);
    await fixture.waitForOwner(conversation.id);
    const readback = await readOtherAM(ctx, fixture, {method: 'GET',
      path: '/api/v1/conversations/' + segment(conversation.id), agentId: conversation.id}, creationReceipt);
    ctx.assert(equal(readback.body, conversation), 'new_conversation_cross_am_readback');
    return conversation;
  }
  async function unsupported(method, suffix, field, actor = 'user') {
    const reply = await request(ctx, method, '/api/v1/responses/' + segment(completed.id) + suffix, undefined,
      {...opts(), actor, allowed: [422]});
    ctx.assert(reply.body.code === 'responses_field_unsupported' && reply.body.field === field,
      'responses_documented_unsupported_operation');
  }
  async function sandbox() {
    const body = (await request(ctx, 'GET', '/api/v1/agui/agents/' + segment(fixture.agentId) + '/sandbox', undefined, opts())).body;
    ctx.assert(body.threadId === fixture.agentId && body.sandbox.id === fixture.sandboxId
      && body.workspace.id === fixture.workspaceId && body.worktree.id === fixture.worktreeId, 'agui_sandbox_identity');
  }
  async function capabilities() {
    const reply = await request(ctx, 'GET', '/api/v1/agui/capabilities');
    ctx.assert(reply.body.protocol === 'ag-ui' && equal(reply.body.transports, ['sse'])
      && reply.body.features.transport.streaming === true && reply.body.features.transport.resumable === true
      && reply.body.features.custom['cma:terminal'] === false, 'agui_advertised_subset');
  }
  async function launchEvents() {
    const reply = await request(ctx, 'GET', '/api/v1/agui/launches/' + segment(fixture.launchId) + '/events?follow=false');
    const frames = sseFrames(reply);
    ctx.assert(equal(frames.map(value => value.body), fixture.launchEvents), 'launch_sse_exact_durable_events');
    let previous = 0;
    for (const frame of frames) {
      ctx.assert(Number(frame.id) === frame.body.seq && frame.body.seq > previous, 'launch_sse_cursor_order');
      previous = frame.body.seq;
    }
  }
  async function readConversation(version = 'v1') {
    const value = (await request(ctx, 'GET', '/api/' + version + '/conversations/' + segment(fixture.agentId), undefined, opts())).body;
    ctx.assert(value.id === fixture.agentId && value.object === 'conversation', 'conversation_same_agent_identity');
  }
  await ctx.step('step_1', async () => {
    fixture = await take(ctx, 'compatibility');
    before = await fixture.readNativeFacts();
    if (ctx.phase === 'after_restart') {
      const prior = ctx.runState.compatibilityResponse;
      ctx.assert(prior?.agentId === fixture.agentId, 'compatibility_retained_agent_required');
      const survived = await request(ctx, 'GET', '/api/v1/responses/' + segment(prior.response.id), undefined, opts());
      ctx.assert(equal(survived.body, prior.response), 'original_response_survives_restart');
    }
    await newConversation();
    await capabilities(); await sandbox(); await launchEvents();
    await readConversation(); await readConversation('v2');
    responseIntent = {conversation: fixture.agentId, input: fixture.responseInput, stream: true};
    const responseKey = key();
    ctx.state.responseKey = responseKey;
    responseReceipt = await request(ctx, 'POST', '/api/v1/responses', responseIntent, {...opts(), key: responseKey});
    completed = assertResponses(ctx, responseReceipt, fixture.agentId, fixture.responseText);
    const read = await request(ctx, 'GET', '/api/v1/responses/' + segment(completed.id), undefined, opts());
    ctx.assert(equal(read.body, completed), 'responses_exact_id_retrieval');
    await unsupported('DELETE', '', 'delete'); await unsupported('POST', '/cancel', 'background');
    aguiIntent = {threadId: fixture.agentId, runId: key(), messages: [{role: 'user', content: fixture.aguiInput}]};
    ctx.state.aguiKey = key();
    agui = assertAgui(ctx, await request(ctx, 'POST', '/api/v1/agui/run', aguiIntent,
      {...opts(), key: ctx.state.aguiKey}), fixture.agentId, fixture.aguiText);
    const replay = await request(ctx, 'GET', '/api/v1/agui/events?thread_id=' + segment(fixture.agentId) + '&follow=false', undefined, opts());
    assertAgui(ctx, replay, fixture.agentId, fixture.aguiText, agui.runId);
    for (const version of ['v1','v2']) {
      const items = await conversationItems(ctx, fixture.agentId, version);
      ctx.assert(items.some(item => item.role === 'assistant' && item.content?.some(part => part.text === fixture.responseText)),
        'conversation_contains_response_output');
    }
    // The v1 cancel alias is unsupported; exercise the documented Agent
    // interrupt on a real running turn and wait for its incomplete response.
    await fixture.holdNextTurn();
    const pending = request(ctx, 'POST', '/api/v1/responses',
      {conversation: fixture.agentId, input: fixture.interruptInput, stream: true}, {...opts(), key: key()});
    let streamSettled = false;
    const outcome = pending.then(value => ({value}), error => ({error}));
    ctx.defer('release_held_compatibility_turn', async () => {
      await fixture.releaseHeldTurn();
      if (!streamSettled) { const value = await outcome; if (value.error) throw value.error; }
    });
    await fixture.waitForHeldTurnStarted(fixture.agentId);
    const interrupt = await request(ctx, 'POST', agentPath('/interrupt'), undefined, {...opts(), key: key(), allowed: [202]});
    ctx.assert(interrupt.status === 202, 'agent_interrupt_admitted');
    await fixture.releaseHeldTurn();
    const result = await outcome;
    if (result.error) throw result.error;
    streamSettled = true;
    cancelled = assertResponses(ctx, result.value, fixture.agentId, undefined, 'incomplete');
    ctx.assert(cancelled.id !== completed.id, 'interrupt_targets_new_response');
    if (ctx.phase === 'before_restart') ctx.runState.compatibilityResponse = {agentId: fixture.agentId, response: completed};
  });
  await ctx.step('step_2', async () => {
    if (number === 24) await sandbox();
    else if (number === 25) await capabilities();
    else if (number === 26) assertAgui(ctx, await request(ctx, 'GET', '/api/v1/agui/events?thread_id='
      + segment(fixture.agentId) + '&follow=false', undefined, opts()), fixture.agentId, fixture.aguiText, agui.runId);
    else if (number === 27) await launchEvents();
    else if (number === 28) {
      const replay = assertAgui(ctx, await request(ctx, 'POST', '/api/v1/agui/run', aguiIntent,
        {...opts(), key: ctx.state.aguiKey}), fixture.agentId, fixture.aguiText, agui.runId);
      ctx.assert(replay.runId === agui.runId, 'agui_same_intent_no_new_run');
    } else if (number === 30) await newConversation();
    else if (number === 31 || number === 84) await readConversation(number === 84 ? 'v2' : 'v1');
    else if (number === 32 || number === 85) {
      const items = await conversationItems(ctx, fixture.agentId, number === 85 ? 'v2' : 'v1');
      ctx.assert(items.some(item => item.role === 'assistant'), 'exact_conversation_output_present');
    } else if (number === 49) {
      const replay = assertResponses(ctx, await request(ctx, 'POST', '/api/v1/responses', responseIntent,
        {...opts(), am: otherAM(responseReceipt.timing.am), key: ctx.state.responseKey}), fixture.agentId, fixture.responseText);
      ctx.assert(replay.id === completed.id, 'responses_same_intent_no_new_response');
    } else if (number === 50) ctx.assert(equal((await request(ctx, 'GET', '/api/v1/responses/' + segment(completed.id),
      undefined, opts())).body, completed), 'exact_response_read');
    else await unsupported(number === 51 ? 'DELETE' : 'POST', number === 51 ? '' : '/cancel', number === 51 ? 'delete' : 'background');
  });
  await ctx.step('step_3', async () => {
    const deniedRequests = [
      ['GET','/api/v1/conversations/' + segment(fixture.agentId)],
      ['GET','/api/v1/conversations/' + segment(fixture.agentId) + '/items'],
      ['GET','/api/v2/conversations/' + segment(fixture.agentId)],
      ['GET','/api/v2/conversations/' + segment(fixture.agentId) + '/items'],
      ['GET','/api/v1/responses/' + segment(completed.id)],
      ['POST','/api/v1/responses', responseIntent],
      ['POST','/api/v1/agui/run', aguiIntent],
      ['GET','/api/v1/agui/agents/' + segment(fixture.agentId) + '/sandbox'],
      ['GET','/api/v1/agui/events?thread_id=' + segment(fixture.agentId) + '&follow=false'],
    ];
    for (const [method,path,body] of deniedRequests) {
      const denied = await request(ctx, method, path, body,
        {...opts(), actor: 'outsider', am: otherAM(ctx.am), key: method === 'POST' ? key() : undefined, allowed: [403,404]});
      ctx.assert(typeof denied.body.code === 'string' && !('output' in denied.body) && !('data' in denied.body), 'compatibility_outsider_denied');
    }
    const deniedCreate = await request(ctx, 'POST', '/api/v1/conversations', fixture.conversationRequest,
      {actor: 'outsider', allowed: [403,404]});
    ctx.assert(typeof deniedCreate.body.code === 'string', 'conversation_foreign_parent_denied');
    const deniedLaunch = await request(ctx, 'GET', '/api/v1/agui/launches/' + segment(fixture.launchId) + '/events?follow=false',
      undefined, {actor: 'outsider', allowed: [404]});
    ctx.assert(typeof deniedLaunch.body.code === 'string', 'launch_stream_owner_boundary');
    await unsupported('DELETE', '', 'delete', 'outsider'); await unsupported('POST', '/cancel', 'background', 'outsider');
    const peer = await readOtherAM(ctx, fixture, {method: 'GET', path: '/api/v1/responses/' + segment(completed.id),
      agentId: fixture.agentId, allowed: [200]}, responseReceipt);
    ctx.assert(equal(peer.body, completed), 'response_cross_am_unchanged_after_refusals');
  });
  await ctx.step('expected_result', async () => {
    const facts = await ctx.observe('compatibility_native_effects', () => fixture.readNativeFacts());
    ctx.assert(facts.input_effects === before.input_effects + 3, 'compatibility_three_intents_no_replay');
    ctx.assert(facts.interrupt_effects === before.interrupt_effects + 1 && facts.active_turns === 0, 'compatibility_interrupt_once_settled');
    const newIds = facts.agent_ids.filter(id => !before.agent_ids.includes(id)).sort();
    ctx.assert(equal(newIds, created.map(value => value.id).sort()), 'compatibility_no_untracked_children');
    const final = await request(ctx, 'GET', '/api/v1/responses/' + segment(cancelled.id), undefined, opts());
    ctx.assert(final.body.status === 'incomplete' && final.body.incomplete_details?.reason === 'interrupted', 'compatibility_interrupt_durable');
  });
}));
