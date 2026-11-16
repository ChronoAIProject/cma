import {define, openBrowser, acquire, egRequest, invoke, visible, uiReceipt, goto, peer, segment, same, digest, pages, poll} from './helpers.mjs';
import {channelCapabilitiesCase} from './channel-capabilities.mjs';

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
  define(20, 'browser', ['browser', 'user-fixtures'], async ctx => {
    let lease, session, record, replay;
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, 'channel_create'); session = await browserChannel(ctx, lease);
      const body = lease.createBody; ctx.assert(body.record_id === lease.recordId && body.name === lease.channelName,
        'channel_create_reserved_intent');
      record = await egRequest(ctx, lease, {method: 'POST', path: '/v1/channels', body, key: lease.keys.create,
        allowed: [201]});
      ctx.assert(record.record_id === lease.recordId && record.definition.creation.ready === true
        && record.channel_id, 'channel_created_durable_record');
    });
    await ctx.step('step_2', async () => {
      replay = await egRequest(ctx, lease, {method: 'POST', path: '/v1/channels', body: lease.createBody,
        key: lease.keys.create, replica: peer(ctx.am), allowed: [201]});
      ctx.assert(replay.channel_id === record.channel_id && replay.provider.binding_id === record.provider.binding_id
        && replay.version === record.version, 'channel_create_replay_same_identity');
      const read = await egRequest(ctx, lease, {path: channel(lease), replica: peer(ctx.am)});
      ctx.assert(read.record_id === record.record_id && read.definition.name === record.definition.name,
        'channel_create_peer_reload');
    });
    await ctx.step('expected_result', async () => {
      ctx.assert(record.definition.owner.subject === lease.expected.ownerSubject, 'channel_owner_identity');
      const counts = await invoke(lease, 'counters'); ctx.assert(counts.turns === lease.baseline.turns, 'channel_create_no_event_turn');
      await ctx.observe('channel_create_receipt', async () => ({record_sha256: digest(record.record_id),
        binding_sha256: digest(record.provider.binding_id), version: record.version})); await uiReceipt(ctx, session.page);
    });
  }),
  define(21, 'eg', ['user-fixtures'], async ctx => {
    let lease, first, recovered;
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, 'channel_create_uncertain');
      first = await invoke(lease, 'createChannelDropResponse', {body: lease.createBody, key: lease.keys.create, replica: ctx.am});
      ctx.assert(first.outcome === 'unknown' && first.recordId === lease.recordId && first.submitAttempts === 1,
        'channel_uncertain_create_retained');
    });
    await ctx.step('step_2', async () => {
      recovered = await egRequest(ctx, lease, {path: `/v1/channel-records/${segment(lease.recordId)}`,
        replica: peer(ctx.am), allowed: [200]});
      ctx.assert(recovered.record_id === lease.recordId && recovered.definition.name === lease.channelName,
        'channel_uncertain_recovered_by_record');
      ctx.assert(recovered.version === lease.expected.recordVersion, 'channel_uncertain_version_observed');
      const changed = await egRequest(ctx, lease, {method: 'POST', path: '/v1/channels', body: {...lease.createBody, name: 'changed'},
        key: lease.keys.create, allowed: [409]});
      ctx.assert(changed.code || changed.type, 'channel_uncertain_changed_intent_refused');
    });
    await ctx.step('expected_result', async () => {
      const counts = await invoke(lease, 'counters'); ctx.assert(counts.createAttempts === 1 && counts.channels === 1,
        'channel_uncertain_no_duplicate_create'); await ctx.observe('channel_recovery', async () => ({record_sha256: digest(recovered.record_id), create_attempts: counts.createAttempts}));
    });
  }),
  define(23, 'browser', ['browser', 'user-fixtures'], async ctx => {
    let lease, session, before, saved;
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, 'producer_edit'); session = await browserChannel(ctx, lease); before = await egRequest(ctx, lease, {path: channel(lease)});
      const body = {...lease.updateBody, sources: lease.updateBody.sources.map((source, i) => i === 0 ? {...source, route_ids: lease.expected.updatedRouteIds} : source),
        expected_version: before.version};
      saved = await save(ctx, lease, body); ctx.assert(saved.version > before.version && saved.definition.sources[0].route_ids,
        'producer_edit_revision_saved');
      ctx.assert(saved.definition.secrets.agent_key_hash === before.definition.secrets.agent_key_hash,
        'producer_edit_preserves_credential_hash');
    });
    await ctx.step('step_2', async () => {
      const stale = await save(ctx, lease, {...lease.updateBody, expected_version: before.version}, [409]);
      ctx.assert(stale.code || stale.type, 'producer_stale_edit_refused');
      const removed = await egRequest(ctx, lease, {method: 'PUT', path: channel(lease), body: {...lease.updateBody,
        sources: saved.definition.sources.filter(source => source.id !== lease.expected.removeSourceId), expected_version: saved.version},
        replica: peer(ctx.am), allowed: [200]});
      ctx.assert(removed.version > saved.version && !removed.definition.sources.some(source => source.id === lease.expected.removeSourceId),
        'producer_selected_source_removed');
    });
    await ctx.step('expected_result', async () => { const current = await egRequest(ctx, lease, {path: channel(lease)});
      ctx.assert(current.definition.secrets.agent_key_hash === before.definition.secrets.agent_key_hash, 'producer_secret_not_replaced'); await uiReceipt(ctx, session.page); });
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
  define(25, 'browser', ['browser', 'user-fixtures'], async ctx => {
    let lease, session, saved;
    await ctx.step('step_1', async () => { lease = await acquire(ctx, 'consumer_profile'); session = await browserChannel(ctx, lease);
      saved = await save(ctx, lease, lease.updateBody); ctx.assert(saved.definition.profile.metadata.name === lease.expected.profileName && saved.provider.slug === lease.expected.providerSlug,
        'consumer_provider_profile_saved'); });
    await ctx.step('step_2', async () => { const read = await egRequest(ctx, lease, {path: channel(lease), replica: peer(ctx.am)});
      ctx.assert(read.definition.profile.permission.action_mode === lease.expected.actionMode && same(read.definition.profile.permission.service_scope, lease.expected.serviceScope), 'consumer_permission_scope_exact');
      ctx.assert(!read.definition.credentials && !JSON.stringify(read).includes(lease.expected.secretMarker), 'consumer_credentials_not_returned'); });
    await ctx.step('expected_result', async () => { const card = await invoke(lease, 'providerCard'); ctx.assert(card.slug === lease.expected.providerSlug && card.allowlisted === true, 'consumer_provider_allowlisted'); await uiReceipt(ctx, session.page); });
  }),
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
