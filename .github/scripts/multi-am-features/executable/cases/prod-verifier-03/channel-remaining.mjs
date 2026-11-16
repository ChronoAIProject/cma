import {Blocked} from '../../runner.mjs';
import {define, openBrowser, acquire, egRequest, invoke, visible, uiReceipt, goto, peer, segment, same, digest, pages, poll} from './helpers.mjs';
import {channelCapabilitiesCase} from './channel-capabilities.mjs';
import {channelCreateCase, channelRecoveryCase} from './channel-creation.mjs';
import {consumerProfileCase} from './consumer-profile.mjs';
import {channelAdmissionCase} from './channel-admission.mjs';
import {channelActivityCase} from './channel-activity.mjs';

const browserChannel = async (ctx, lease) => {
  const session = await openBrowser(ctx, 'user', {agentId: lease.agentId});
  await goto(session.page, '/event-gateway');
  await visible(ctx, session.page.getByRole('heading', {name: lease.channelName, exact: true}), 'channel_browser_open');
  return session;
};

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
  channelAdmissionCase,
  consumerProfileCase,
  // Routing stores handoff policy, but the inspected product has no publisher
  // or hop/rate enforcement path. Do not pass the whole frozen row using only
  // ordinary routing counters, a wrong-endpoint PUT, or adapter-made handoffs.
  define(26, 'browser', ['browser', 'user-fixtures'], async ctx => {
    await ctx.step('step_1', async () => {
      await acquire(ctx, 'multi_consumer_routing');
      throw new Blocked('routing_handoff_runtime_and_limits_missing');
    });
    await ctx.step('step_2', async () => {throw new Blocked('routing_handoff_runtime_and_limits_missing');});
    await ctx.step('expected_result', async () => {throw new Blocked('routing_handoff_runtime_and_limits_missing');});
  }),
  channelActivityCase,
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
