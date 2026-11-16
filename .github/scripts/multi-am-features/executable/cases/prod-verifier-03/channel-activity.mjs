import {Blocked} from '../../runner.mjs';
import {define, acquire, openBrowser, egRequest, invoke, visible, goto, peer, segment, same, digest, uiReceipt} from './helpers.mjs';
import {channelReceipt, openListedChannel} from './channel-browser.mjs';
import {guardDiscoveryWrites} from './channel-capabilities.mjs';
import {assertActivityPage, expiredActivityCursor, activityProjection, assertAtomicActivity, activityWait,
  observeActivityStream, observeActivityDom, overrideNextActivityCursor} from './activity-observer.mjs';

const isText = value => typeof value === 'string' && value.length > 0;
const independentAssert = {assert: (value, code) => {if (!value) throw Error(code);}};

export function validateActivityFixture(lease) {
  const e = lease.expected;
  if (!isText(lease.recordId) || !isText(lease.channelId) || !e
    || !Array.isArray(e.retainedEventIds) || e.retainedEventIds.length < 2 || e.retainedEventIds.length > 20
    || !e.retainedEventIds.every(isText) || new Set(e.retainedEventIds).size !== e.retainedEventIds.length
    || !['eventId', 'consumerId', 'conversationId', 'agentId', 'agentConversationId'].every(key => isText(e.held?.[key]))
    || !e.retainedEventIds.includes(e.held.eventId)
    || !isText(e.unrelated?.agentId) || !isText(e.unrelated?.conversationId)
    || e.unrelated.agentId === e.held.agentId || e.unrelated.conversationId === e.held.agentConversationId
    || !Array.isArray(e.unrelated.markers) || e.unrelated.markers.length < 2
    || !e.unrelated.markers.every(marker => isText(marker) && marker.length >= 16)
    || new Set(e.unrelated.markers).size !== e.unrelated.markers.length)
    throw new Blocked('activity_retained_held_and_unrelated_seed_required');
  for (const method of ['request', 'releaseCompletion', 'drain', 'providerContext'])
    if (typeof lease[method] !== 'function') throw new Blocked('activity_primitive_' + method.toLowerCase());
  return e;
}

export function assertUnrelatedContext(ctx, expected, observed) {
  ctx.assert(observed?.agentId === expected.agentId && observed.conversationId === expected.conversationId
    && Number.isSafeInteger(observed.requestCount) && observed.requestCount > 0
    && Array.isArray(observed.messages) && observed.messages.length > 0, 'activity_unrelated_provider_capture');
  const messages = observed.messages.map(message => typeof message.content === 'string' ? message.content
    : Array.isArray(message.content) ? message.content.map(part => typeof part.text === 'string' ? part.text : '').join('\n') : '').join('\n');
  ctx.assert(expected.markers.every(marker => messages.includes(marker)), 'activity_unrelated_markers_admitted');
}

export function assertHeldActivity(ctx, page, expected, completed) {
  const item = page.items.find(item => item.event_id === expected.eventId);
  ctx.assert(item?.conversation.id === expected.conversationId, 'activity_event_detail_conversation');
  const consumer = item.consumers.find(consumer => consumer.consumer_id === expected.consumerId);
  ctx.assert(consumer?.agent_id === expected.agentId && consumer.conversation_id === expected.agentConversationId
    && isText(consumer.response_id) && Number.isFinite(Date.parse(consumer.dispatched_at))
    && Number.isFinite(Date.parse(consumer.accepted_at)), 'activity_event_detail_provider_identity');
  ctx.assert(consumer.state === (completed ? 'completed' : 'accepted'), 'activity_held_transition_state');
  if (completed) ctx.assert(Number.isFinite(Date.parse(consumer.produced_at))
    && Number.isFinite(Date.parse(consumer.completed_at)) && consumer.deliveries.some(delivery =>
      delivery.step === 'answer' && delivery.outcome === 'confirmed'
      && Number.isFinite(Date.parse(delivery.confirmed_at)) && delivery.recipients?.length > 0),
  'activity_event_detail_confirmed_delivery');
  else ctx.assert(consumer.deliveries.length === 0 && consumer.produced_at == null,
    'activity_completion_is_actually_held');
  return consumer;
}

// Metadata/UI reads need actual EG attribution. The seeded provider work is not
// projected into a fictitious AM event-turn receipt for this read-only row.
export const channelActivityCase = {...define(28, 'browser', ['browser', 'user-fixtures', 'eg_browser'], async ctx => {
  let lease, expected, session, stream, dom, panel, guard, url, first, completed, resumed, reset;
  let firstReceipt, resumedReceipt, resetReceipt;
  const forbidden = () => expected.unrelated.markers;
  const validate = page => assertActivityPage(independentAssert, page, expected.retainedEventIds, forbidden());
  const retainFrame = async frame => {
    // The EG proxy must expose handshake receipts while SSE is still open.
    // Waiting for ordinary network-idle here would wait for the infinite body.
    const receipts = await activityWait(ctx, async () => {
      const evidence = await invoke(session, 'egEvidence');
      return evidence.some(receipt => receipt.request_id === frame.connection.requestId) ? evidence : null;
    }, 'activity_stream_attribution_missing');
    const receipt = channelReceipt(ctx, receipts, frame.connection.requestId);
    await ctx.observe('activity_stream_eg_receipt', async () => ({schema: 'multi-eg-request-v1', receipt}));
    return receipt;
  };
  const rendered = async frame => {
    await activityWait(ctx, async () => {
      stream.check();
      const observations = await dom.read();
      assertAtomicActivity(ctx, observations, stream.frames);
      return same(observations.at(-1).rows, activityProjection(frame.page));
    }, 'activity_snapshot_not_rendered');
    await visible(ctx, panel.getByRole('status').filter({hasText: /^Live activity$/}), 'activity_live_status');
  };
  const reconnect = async (previous, replica, expired) => {
    const index = stream.frames.length, connectionIndex = previous.connection.index;
    const context = session.page.context();
    // Restoration custody is registered before the first local network fault.
    ctx.defer('restore_activity_context_online', () => context.setOffline(false));
    await context.setOffline(true);
    await visible(ctx, panel.getByRole('status').filter({hasText: /^Reconnecting live activity…$/}),
      'activity_native_reconnecting_status');
    await session.switchEG(replica);
    const override = expired ? await overrideNextActivityCursor(ctx, session.page, url, expired) : null;
    await context.setOffline(false);
    const frame = await stream.wait(frame => frame.index >= index && frame.connection.index > connectionIndex,
      'activity_native_reconnect_frame_missing');
    ctx.assert(frame.type === (expired ? 'reset' : 'activity'), 'activity_reconnect_event_kind');
    if (override) {override.check(previous.page.cursor); await override.close();}
    else ctx.assert(frame.connection.lastEventId === previous.page.cursor, 'activity_native_last_event_id');
    ctx.assert(same(frame.page, previous.page), 'activity_reconnect_complete_snapshot');
    const receipt = await retainFrame(frame);
    ctx.assert(receipt.replica === replica, 'activity_reconnect_actual_replica');
    await rendered(frame);
    return {frame, receipt};
  };
  await ctx.step('step_1', async () => {
    lease = await acquire(ctx, 'channel_activity'); expected = validateActivityFixture(lease);
    assertUnrelatedContext(ctx, expected.unrelated, await invoke(lease, 'providerContext', {
      agentId: expected.unrelated.agentId, conversationId: expected.unrelated.conversationId}));
    const channel = await egRequest(ctx, lease, {path: '/v1/channels/' + segment(lease.channelId)});
    ctx.assert(channel.record_id === lease.recordId && channel.channel_id === lease.channelId,
      'activity_fixture_channel_identity');
    const before = await egRequest(ctx, lease, {path: '/v1/channels/' + segment(lease.channelId) + '/activity'});
    assertActivityPage(ctx, before, expected.retainedEventIds, forbidden());
    assertHeldActivity(ctx, before, expected.held, false);
    session = await openBrowser(ctx, 'user', {channelActivity: true});
    for (const method of ['egEvidence', 'switchEG', 'networkIdle'])
      if (typeof session[method] !== 'function') throw new Blocked('activity_actual_eg_browser_required');
    guard = await guardDiscoveryWrites(ctx, session.page);
    url = new URL('/api/event-gateway/v1/channels/' + segment(lease.channelId) + '/activity/stream', session.origin).href;
    stream = await observeActivityStream(ctx, session.page, url, validate);
    await openListedChannel(ctx, session, channel, () => goto(session.page, '/event-gateway'));
    first = await stream.wait(() => true, 'activity_initial_snapshot_missing');
    ctx.assert(first.type === 'activity' && same(first.page, before), 'activity_retained_history_from_native_stream');
    firstReceipt = await retainFrame(first);
    await session.page.getByRole('button', {name: 'Activity', exact: true}).click();
    panel = session.page.getByRole('dialog', {name: 'Channel settings', exact: true})
      .locator('section[aria-label="Activity"] .channel-activity');
    await visible(ctx, panel, 'activity_full_inspector_visible');
    const list = panel.locator('.channel-activity__events');
    await activityWait(ctx, async () => await list.locator(':scope > li').count() === before.items.length,
      'activity_retained_rows_not_rendered');
    dom = await observeActivityDom(ctx, list, session.page, forbidden());
    await rendered(first);
    // The case releases only the already admitted synthetic model completion;
    // all subsequent snapshots and delivery details must come from the product.
    await invoke(lease, 'releaseCompletion', {eventId: expected.held.eventId});
    const finality = await invoke(lease, 'drain', {eventId: expected.held.eventId,
      timeoutMs: Math.max(1, ctx.remainingMs?.() ?? 30000)});
    ctx.assert(finality.pending === 0 && finality.unknown === 0 && finality.lateEffects === 0,
      'activity_seed_completion_finality');
    completed = await stream.wait(frame => frame.index > first.index && frame.page.cursor !== first.page.cursor
      && frame.page.items.find(item => item.event_id === expected.held.eventId)?.consumers.some(c =>
        c.consumer_id === expected.held.consumerId && c.state === 'completed'
        && c.deliveries.some(d => d.step === 'answer' && d.outcome === 'confirmed')), 'activity_real_live_update_missing');
    ctx.assert(completed.connection === first.connection && completed.type === 'activity', 'activity_live_update_same_connection');
    const beforeConsumer = assertHeldActivity(ctx, first.page, expected.held, false);
    const afterConsumer = assertHeldActivity(ctx, completed.page, expected.held, true);
    ctx.assert(beforeConsumer.response_id === afterConsumer.response_id, 'activity_completion_response_identity');
    await rendered(completed);
  });
  await ctx.step('step_2', async () => {
    const reconnected = await reconnect(completed, peer(firstReceipt.replica));
    resumed = reconnected.frame; resumedReceipt = reconnected.receipt;
    ctx.assert(resumedReceipt.serving.instance_id !== firstReceipt.serving.instance_id
      && resumedReceipt.serving.pod_uid !== firstReceipt.serving.pod_uid, 'activity_two_actual_eg_processes');
    const expired = expiredActivityCursor(resumed.page);
    ctx.assert(expired !== resumed.page.cursor, 'activity_expired_cursor_changes_only_request');
    const resetResult = await reconnect(resumed, firstReceipt.replica, expired);
    reset = resetResult.frame; resetReceipt = resetResult.receipt;
    assertHeldActivity(ctx, reset.page, expected.held, true);
  });
  await ctx.step('expected_result', async () => {
    const durable = await egRequest(ctx, lease, {path: '/v1/channels/' + segment(lease.channelId) + '/activity',
      replica: resumedReceipt.replica});
    assertActivityPage(ctx, durable, expected.retainedEventIds, forbidden());
    ctx.assert(same(durable, reset.page), 'activity_reset_matches_retained_truth');
    await rendered(reset);
    const observations = await dom.read();
    assertAtomicActivity(ctx, observations, stream.frames);
    ctx.assert(observations.length >= 2 && guard.attempts() === 0, 'activity_changed_atomic_views_without_browser_writes');
    ctx.assert(stream.frames.every(frame => frame.connection.requestId), 'activity_every_frame_has_actual_connection');
    // Join every observed connection, including intermediate updates. Do not
    // treat requested replica labels or raw EG receipts as AM ownership proof.
    for (const connection of new Set(stream.frames.map(frame => frame.connection)))
      await retainFrame(stream.frames.find(frame => frame.connection === connection));
    await ctx.observe('activity_native_snapshots', async () => ({schema: 'multi-eg-activity-v1',
      frames: stream.frames.map(frame => ({type: frame.type, request_sha256: digest(frame.connection.requestId),
        cursor_sha256: digest(frame.page.cursor), snapshot_sha256: digest(frame.page), items: frame.page.items.length})),
      rendered_snapshots: observations.map(value => digest(value.rows)),
      unrelated_markers_checked: forbidden().length,
      actual_replicas: [firstReceipt.replica, resumedReceipt.replica, resetReceipt.replica]}));
    await stream.close();
    await uiReceipt(ctx, session.page);
  });
}), evidence_scope: 'eg_only'};
