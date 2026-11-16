import {Blocked} from '../../runner.mjs';
import {define, openBrowser, acquire, egRequest, invoke, visible, uiReceipt, goto, peer, segment, same, digest, pages, poll} from './helpers.mjs';
import {channelCapabilitiesCase} from './channel-capabilities.mjs';
import {channelCreateCase, channelRecoveryCase} from './channel-creation.mjs';
import {consumerProfileCase} from './consumer-profile.mjs';

const channel = lease => `/v1/channels/${segment(lease.channelId)}`;
const browserChannel = async (ctx, lease) => {
  const session = await openBrowser(ctx, 'user', {agentId: lease.agentId});
  await goto(session.page, '/event-gateway');
  await visible(ctx, session.page.getByRole('heading', {name: lease.channelName, exact: true}), 'channel_browser_open');
  return session;
};
const save = async (ctx, lease, body, expected = [200]) => egRequest(ctx, lease, {
  method: 'PUT', path: channel(lease), body, replica: ctx.am, allowed: expected,
});

export const channelCases = [
  channelCapabilitiesCase,
  channelCreateCase,
  channelRecoveryCase,
  // The frozen EG-023 includes removing a producer in the web editor.
  // NodeFields/ChannelMap expose add/edit only; Source has no generic id field.
  // Keep this row explicitly incomplete instead of substituting a direct PUT
  // or treating an unchanged sources array as successful UI removal.
  define(23, 'browser', ['browser', 'user-fixtures'], async ctx => {
    await ctx.step('step_1', async () => {
      await acquire(ctx, 'producer_edit');
      throw new Blocked('producer_removal_web_workflow_missing');
    });
    await ctx.step('step_2', async () => {throw new Blocked('producer_removal_web_workflow_missing');});
    await ctx.step('expected_result', async () => {throw new Blocked('producer_removal_web_workflow_missing');});
  }),
  define(24, 'browser', ['browser', 'user-fixtures'], async ctx => {
    let lease, session, saved, events;
    await ctx.step('step_1', async () => { lease = await acquire(ctx, 'channel_admission'); session = await browserChannel(ctx, lease);
      saved = await save(ctx, lease, {...lease.updateBody, partition: lease.expected.partition, reply: lease.expected.reply}, [200]);
      ctx.assert(saved.definition.partition.key === lease.expected.partition.key && saved.definition.reply.auto_final === lease.expected.reply.auto_final,
        'admission_grouping_saved'); });
    await ctx.step('step_2', async () => { events = [];
      for (const variant of ['admitted_sender_one','admitted_sender_two','denied_sender']) {
        const event = await invoke(lease, 'platformSend', {platform: lease.platform, variant, replica: variant === 'admitted_sender_one' ? ctx.am : peer(ctx.am)});
        ctx.assert(['accepted','refused','rejected','ignored'].includes(event.status ?? event.outcome), 'admission_outcome_explicit'); events.push(event);
      }
      const turns = await invoke(lease, 'counters'); ctx.assert(turns.turns === lease.baseline.turns + 2 && turns.effects === lease.baseline.effects + 2,
        'admission_only_allowed_turns'); });
    await ctx.step('expected_result', async () => { const partition = await egRequest(ctx, lease, {path: `/v1/channels/${segment(lease.channelId)}/partition?partition_key=${encodeURIComponent(lease.expected.partition.key)}`});
      ctx.assert(partition.grouping === lease.expected.partition.key && partition.conversations.length === 2, 'partition_grouping_exact'); await uiReceipt(ctx, session.page); });
  }),
  consumerProfileCase,
  define(26, 'browser', ['browser', 'user-fixtures'], async ctx => {
    let lease, session, saved;
    await ctx.step('step_1', async () => { lease = await acquire(ctx, 'multi_consumer_routing'); session = await browserChannel(ctx, lease);
      saved = await save(ctx, lease, lease.routingBody); ctx.assert(saved.definition.routing.default_targets.length === lease.expected.defaultTargets.length && saved.definition.routing.max_targets === lease.expected.maxTargets, 'routing_limits_saved'); });
    await ctx.step('step_2', async () => { for (const variant of ['matched_rule','unmatched_event','over_limit']) { const event = await invoke(lease, 'platformSend', {variant, replica: ctx.am}); ctx.assert(['accepted','refused','rejected','ignored'].includes(event.status ?? event.outcome), 'routing_variant_explicit'); }
      const counts = await invoke(lease, 'counters'); ctx.assert(counts.duplicateDeliveries === 0 && counts.maxTargetsObserved <= lease.expected.maxTargets, 'routing_limits_effective'); });
    await ctx.step('expected_result', async () => { const current = await egRequest(ctx, lease, {path: channel(lease), replica: peer(ctx.am)}); ctx.assert(current.definition.routing.version === saved.definition.routing.version, 'routing_peer_revision'); await uiReceipt(ctx, session.page); });
  }),
  define(28, 'browser', ['browser', 'user-fixtures'], async ctx => {
    let lease, session, before;
    await ctx.step('step_1', async () => { lease = await acquire(ctx, 'channel_activity'); session = await browserChannel(ctx, lease); before = await invoke(lease, 'activitySnapshot'); await session.page.getByRole('button', {name: 'Activity', exact: true}).click(); await visible(ctx, session.page.getByRole('heading', {name: /Activity/}), 'activity_open'); });
    await ctx.step('step_2', async () => { const live = await invoke(lease, 'activityStream'); ctx.assert(live.cursor && Array.isArray(live.items), 'activity_live_cursor'); const detail = await invoke(lease, 'activityDetail', {eventKey: lease.expected.eventKey}); ctx.assert(detail.event_id === lease.expected.eventId && detail.conversation.id === lease.expected.conversationId, 'activity_detail_identity'); const reset = await invoke(lease, 'activityStream', {cursor: lease.expected.expiredCursor}); ctx.assert(reset.reset === true || reset.code === 'cursor_expired', 'activity_expired_cursor_explicit'); });
    await ctx.step('expected_result', async () => { const after = await invoke(lease, 'activitySnapshot'); ctx.assert(after.items.length >= before.items.length && new Set(after.items.map(item => item.event_id)).size === after.items.length, 'activity_snapshot_atomic_no_duplicates'); await uiReceipt(ctx, session.page); });
  }),
  define(29, 'browser', ['browser', 'user-fixtures'], async ctx => {
    let lease, session, receipt;
    await ctx.step('step_1', async () => { lease = await acquire(ctx, 'outbound_prepare'); session = await browserChannel(ctx, lease); const body = lease.outboundBody; receipt = await egRequest(ctx, lease, {method: 'POST', path: `/v1/channels/${segment(lease.channelId)}/outbound-messages`, body, key: lease.keys.prepare}); ctx.assert(receipt.status === 'prepared' && receipt.delivery_id && receipt.delivery?.outcome === 'not_sent', 'outbound_prepared_not_sent'); });
    await ctx.step('step_2', async () => { const read = await egRequest(ctx, lease, {path: `/v1/channels/${segment(lease.channelId)}/outbound-messages/${segment(receipt.delivery_id)}`, replica: peer(ctx.am)}); ctx.assert(read.delivery_id === receipt.delivery_id && read.body.route_id === lease.outboundBody.route_id && read.body.message.native_attachments?.length <= 4, 'outbound_receipt_exact_content'); const effects = await invoke(lease, 'captureEffects', {deliveryId: receipt.delivery_id}); ctx.assert(effects.length === 0, 'outbound_prepare_no_delivery_effect'); });
    await ctx.step('expected_result', async () => { const status = await invoke(lease, 'outboundStatus', {deliveryId: receipt.delivery_id}); ctx.assert(status.outcome === 'prepared' && status.sent === false, 'outbound_preparation_state'); await uiReceipt(ctx, session.page); });
  }),
  define(30, 'browser', ['browser', 'user-fixtures'], async ctx => {
    let lease, session, receipt;
    await ctx.step('step_1', async () => { lease = await acquire(ctx, 'outbound_send'); session = await browserChannel(ctx, lease); receipt = await egRequest(ctx, lease, {method: 'POST', path: `/v1/channels/${segment(lease.channelId)}/outbound-messages`, body: lease.outboundBody, key: lease.keys.prepare}); ctx.assert(receipt.status === 'prepared', 'outbound_send_prepared'); const sent = await egRequest(ctx, lease, {method: 'POST', path: `/v1/channels/${segment(lease.channelId)}/outbound-messages/${segment(receipt.delivery_id)}/send`, body: lease.outboundBody.message, key: lease.keys.send}); ctx.assert(sent.delivery?.outcome === 'confirmed' && sent.delivery_id === receipt.delivery_id, 'outbound_sent_exact_receipt'); });
    await ctx.step('step_2', async () => { const read = await egRequest(ctx, lease, {path: `/v1/channels/${segment(lease.channelId)}/outbound-messages/${segment(receipt.delivery_id)}`, replica: peer(ctx.am)}); ctx.assert(read.delivery?.outcome === 'confirmed' && read.delivery_id === receipt.delivery_id, 'outbound_peer_delivery_read'); const uncertain = await invoke(lease, 'dropOutboundSendResponse', {deliveryId: lease.expected.uncertainDeliveryId, key: lease.keys.uncertain}); ctx.assert(uncertain.outcome === 'unknown', 'outbound_uncertain_explicit'); const recovered = await invoke(lease, 'outboundStatus', {deliveryId: lease.expected.uncertainDeliveryId}); ctx.assert(recovered.sendAttempts === 1 && recovered.outcome === 'confirmed', 'outbound_uncertain_reconciled_no_resend'); });
    await ctx.step('expected_result', async () => { const cancel = await invoke(lease, 'cancelOutbound', {deliveryId: lease.expected.cancelDeliveryId, key: lease.keys.cancel}); ctx.assert(cancel.status === 'cancelled' && cancel.sent === false, 'outbound_cancel_unsent_only'); const final = await invoke(lease, 'drain', {timeoutMs: 90000}); ctx.assert(final.unknown === 0 && final.pending === 0 && final.sendAttempts === 2, 'outbound_finality_no_duplicate'); await uiReceipt(ctx, session.page); });
  }),
];
