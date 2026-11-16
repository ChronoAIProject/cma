import {define, acquire, invoke, peer, segment, same, digest, eventJoin} from './helpers.mjs';
import {send} from './platform.mjs';

const bindingPath = lease => `/api/v1/cmaeg/bindings/${segment(lease.bindingId)}`;
async function amRead(ctx, lease, path, am = ctx.am, actor = 'gateway_service') {
  const response = await ctx.request({method: 'GET', path, am, actor, agentId: lease.agentId});
  ctx.assert(response.timing.am === am, 'binding_actual_requested_am');
  return response.body;
}

export const bindingCases = [
  define(57, 'eg', ['eg'], async ctx => {
    let lease, first, followup, different;
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, 'binding_partitions');
      first = await send(ctx, lease, {platform: 'custom', trustMode: 'custom', conversation: 'one',
        sender: 'owner_one', replica: ctx.am, text: 'remember synthetic first partition'});
      followup = await send(ctx, lease, {platform: 'custom', trustMode: 'custom', conversation: 'one',
        sender: 'owner_one', replica: peer(ctx.am), text: 'repeat first partition'});
      different = await send(ctx, lease, {platform: 'custom', trustMode: 'custom', conversation: 'two',
        sender: 'owner_one', replica: peer(ctx.am), text: 'new partition'});
      ctx.assert(first.event.conversationId === followup.event.conversationId
        && first.event.agentId === followup.event.agentId, 'binding_same_partition_context');
      ctx.assert(different.event.conversationId !== first.event.conversationId
        && different.event.agentId !== first.event.agentId, 'binding_different_partition_isolated');
      ctx.assert(followup.receipt.contextEventIds.includes(first.event.eventId)
        && !different.receipt.contextEventIds.includes(first.event.eventId), 'binding_followup_context_isolation');
    });
    await ctx.step('step_2', async () => {
      const current = await amRead(ctx, lease, bindingPath(lease));
      ctx.assert(current.binding_id === first.event.bindingId && current.generation === first.event.bindingGeneration,
        'binding_exact_generation');
      const denied = await ctx.request({method: 'DELETE', actor: 'outsider', path: bindingPath(lease),
        agentId: lease.agentId, allowed: [403, 404]});
      ctx.assert(denied.status !== 204, 'binding_delete_authorized_only');
      const deleted = await ctx.request({method: 'DELETE', actor: 'gateway_service', path: bindingPath(lease),
        agentId: lease.agentId, allowed: [204]});
      ctx.assert(deleted.status === 204, 'binding_deleted_publicly');
      const blocked = await invoke(lease, 'submitFrozenBindingEvent', {event: first.event, intent: 'new_followup', replica: peer(ctx.am)});
      ctx.assert(['refused', 'rejected'].includes(blocked.outcome) && blocked.turnCount === 0,
        'retired_binding_not_reused');
    });
    await ctx.step('expected_result', async () => {
      const foreign = await invoke(lease, 'crossOwnerConversation', {conversationId: first.event.conversationId,
        actor: 'other_owner'});
      ctx.assert(foreign.status === 403 || foreign.status === 404, 'binding_foreign_owner_context_denied');
      const final = await invoke(lease, 'drain', {timeoutMs: 90000});
      ctx.assert(final.turns === 3 && final.pending === 0 && final.unknown === 0, 'binding_no_post_unbind_turn');
      await ctx.observe('binding_partition_receipt', async () => ({first: digest(first.event.conversationId),
        followup: digest(followup.event.conversationId), separate: digest(different.event.conversationId), counts: final}));
    });
  }),
  define(58, 'eg', ['eg'], async ctx => {
    let lease, event, record;
    const snapshots = [];
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, 'channel_attribution');
      event = (await send(ctx, lease, {platform: 'custom', trustMode: 'custom', replica: ctx.am,
        conversation: 'attributed', text: 'synthetic channel usage'})).event;
      for (const am of [ctx.am, peer(ctx.am)]) {
        const current = await amRead(ctx, lease, `/api/v1/gateway/channel-records/${segment(lease.recordId)}`, am);
        ctx.assert(current.record_id === lease.recordId && current.version === lease.expected.recordVersion,
          'gateway_record_exact_durable_revision');
        if (record) ctx.assert(same(current, record), 'gateway_record_cross_am_converges'); record = current;
      }
    });
    await ctx.step('step_2', async () => {
      const base = `/api/v1/agents/${segment(event.agentId)}/channel-usage`;
      for (const am of [ctx.am, peer(ctx.am)]) {
        const current = await amRead(ctx, lease, base, am, 'user');
        ctx.assert(current.agent_id === event.agentId && current.channel_usage.active === true
          && current.channel_usage.has_history === true, 'channel_usage_current_summary');
        ctx.assert(current.bindings.some(b => b.binding_id === event.bindingId && b.generation === event.bindingGeneration),
          'channel_usage_binding_generation'); snapshots.push(current);
      }
      await ctx.request({method: 'DELETE', path: bindingPath(lease), actor: 'gateway_service',
        agentId: event.agentId, allowed: [204]});
      const history = await amRead(ctx, lease, base + '/history', peer(ctx.am), 'user');
      ctx.assert(history.agent_id === event.agentId && ['attach', 'use', 'detach'].every(kind =>
        history.items.some(item => item.change.kind === kind && item.change.binding_id === event.bindingId)),
      'channel_usage_attach_use_detach_history');
      ctx.assert(history.items.every((item, i) => i === 0 || item.revision === history.items[i - 1].revision + 1),
        'channel_usage_contiguous_revisions');
    });
    await ctx.step('expected_result', async () => {
      const safe = await amRead(ctx, lease, `/api/v1/agents/${segment(event.agentId)}/channel-usage`, ctx.am, 'shared_reader');
      ctx.assert(safe.agent_id === event.agentId && safe.channel_usage.has_history === true
        && safe.channel_usage.active === false && !('bindings' in safe), 'shared_reader_safe_detached_summary');
      const denied = await ctx.request({method: 'GET', actor: 'shared_reader', agentId: event.agentId,
        path: `/api/v1/agents/${segment(event.agentId)}/channel-usage/history`, allowed: [403]});
      ctx.assert(denied.status === 403, 'shared_reader_no_management_history');
      await ctx.observe('channel_usage_snapshot_hashes', async () => snapshots.map(digest));
    });
  }),
  define(59, 'eg', ['eg'], async ctx => {
    let lease, before, prepared;
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, 'provider_preparation'); before = await invoke(lease, 'counters');
      const discovery = await invoke(lease, 'providerCard');
      ctx.assert(discovery.slug === lease.expected.providerSlug && discovery.allowlisted === true
        && discovery.card.url === lease.expected.providerUrl, 'provider_allowlisted_agent_card');
      ctx.assert(discovery.capabilities.includes('binding_preparation'), 'provider_preparation_capability');
      ctx.assert((await invoke(lease, 'counters')).turns === before.turns, 'provider_discovery_no_turn_claim');
    });
    await ctx.step('step_2', async () => {
      for (const [name, classification] of [['knownWorkerDigest', 'known_worker'], ['unknownWorkerDigest', 'unknown']]) {
        const response = await ctx.request({method: 'POST', actor: 'gateway_service',
          path: '/api/v1/cmaeg/worker-credentials/classify', body: {sha256: lease.expected[name]}});
        ctx.assert(same(response.body, {classification}), 'worker_credential_classification_exact');
      }
      const denied = await ctx.request({method: 'POST', actor: 'user', path: '/api/v1/cmaeg/worker-credentials/classify',
        body: {sha256: lease.expected.knownWorkerDigest}, allowed: [403]});
      ctx.assert(denied.status === 403, 'worker_classification_service_only');
      const inspection = await invoke(lease, 'inspectPreparation', {bindingId: lease.bindingId, actor: 'gateway_service'});
      ctx.assert(inspection.state === 'pending' && inspection.intent.binding_id === lease.bindingId
        && inspection.intent.generation === lease.expected.generation
        && inspection.intent.owner === lease.expected.owner, 'preparation_exact_generation_intent');
      prepared = await invoke(lease, 'prepareBinding', {bindingId: lease.bindingId, intent: inspection.intent,
        key: lease.keys.prepare, replica: ctx.am});
      const replay = await invoke(lease, 'prepareBinding', {bindingId: lease.bindingId, intent: inspection.intent,
        key: lease.keys.prepare, replica: peer(ctx.am)});
      ctx.assert(same(prepared, replay) && prepared.generation === inspection.intent.generation,
        'preparation_exact_replay_receipt');
      const wrong = await invoke(lease, 'prepareBinding', {bindingId: lease.bindingId,
        intent: {...inspection.intent, generation: inspection.intent.generation + 1},
        key: lease.keys.prepare, replica: peer(ctx.am)});
      ctx.assert(wrong.status === 409 || wrong.status === 403, 'preparation_changed_generation_refused');
    });
    await ctx.step('expected_result', async () => {
      const after = await invoke(lease, 'counters');
      ctx.assert(after.turns === before.turns && after.preparations === before.preparations + 1,
        'preparation_no_second_turn_or_allocation');
      await ctx.observe('provider_preparation_hash', async () => ({receipt_sha256: digest(prepared), before, after}));
    });
  }),
  define(79, 'agent', ['eg'], async ctx => {
    let lease, event, artifact, base, uploaded, supplement;
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, 'am_media_supplement');
      event = await invoke(lease, 'platformSend', {platform: 'telegram', variant: 'frozen_attachment', replica: ctx.am});
      artifact = event.artifact;
      ctx.assert(artifact.attachment_index === event.attachmentIndex && /^[a-f0-9]{64}$/.test(artifact.content_sha256)
        && artifact.content_sha256 === lease.expected.contentDigest && artifact.size_bytes === lease.expected.contentBytes,
      'am_media_frozen_artifact_identity');
      ctx.assert(event.bindingGeneration === lease.expected.generation && event.agentId === lease.agentId,
        'am_media_original_target_generation');
      base = `${bindingPath(lease)}/conversations/${segment(event.conversationId)}/events/${segment(event.eventId)}`
        + `/media/${event.attachmentIndex}`;
    });
    await ctx.step('step_2', async () => {
      const read = await ctx.request({method: 'GET', path: base + '/attachment', actor: 'bound_consumer',
        agentId: event.agentId, am: ctx.am, allowed: [200, 404]});
      if (read.status === 200) uploaded = read.body;
      else {
        const uncertain = await invoke(lease, 'uploadFrozenAttachment', {path: base + '/attachment',
          artifact, generation: event.bindingGeneration, expiresAt: event.expiresAt, dropResponse: true});
        ctx.assert(uncertain.outcome === 'unknown', 'am_media_upload_uncertainty_retained');
        uploaded = (await ctx.request({method: 'GET', path: base + '/attachment', actor: 'bound_consumer',
          agentId: event.agentId, am: peer(ctx.am)})).body;
      }
      ctx.assert(uploaded.agent_id === event.agentId && uploaded.sha256 === artifact.content_sha256
        && uploaded.size_bytes === artifact.size_bytes && uploaded.name === artifact.name, 'am_media_receipt_matches_frozen_upload');
      const body = {gateway_binding_generation: event.bindingGeneration, event_expires_at: event.expiresAt,
        artifact, derived_text: lease.expected.derivedText};
      supplement = (await ctx.request({method: 'POST', path: base + '/supplement', body,
        actor: 'bound_consumer', agentId: event.agentId, allowed: [202]})).body;
      const replay = (await ctx.request({method: 'POST', path: base + '/supplement', body,
        actor: 'bound_consumer', agentId: event.agentId, am: peer(ctx.am), allowed: [202]})).body;
      ctx.assert(replay.command_id === supplement.command_id && replay.response_id === supplement.response_id,
        'am_media_supplement_replays_exact_response');
      for (const changed of [{...body, gateway_binding_generation: event.bindingGeneration + 1},
        {...body, artifact: {...artifact, content_sha256: '0'.repeat(64)}}, {...body, derived_text: 'changed frozen text'}]) {
        const denied = await ctx.request({method: 'POST', path: base + '/supplement', body: changed,
          actor: 'bound_consumer', agentId: event.agentId, allowed: [400, 403, 409, 422]});
        ctx.assert(denied.status !== 202, 'am_media_changed_frozen_intent_refused');
      }
    });
    await ctx.step('expected_result', async () => {
      ctx.assert(supplement.response_id && supplement.response_id !== event.responseId,
        'am_media_continuation_distinct_response');
      const outcome = await invoke(lease, 'continuationReceipts', {eventId: event.eventId});
      ctx.assert(outcome.length === 1 && outcome[0].responseId === supplement.response_id
        && outcome[0].attachmentIndex === event.attachmentIndex, 'am_media_single_exact_continuation');
      const counts = await invoke(lease, 'counters');
      ctx.assert(counts.uploadAttempts <= 1 && counts.continuations === 1, 'am_media_unknown_upload_not_retried');
      await ctx.observe('am_media_identity_receipt', async () => ({artifact_sha256: artifact.content_sha256,
        attachment_sha256: digest(uploaded.id), response_sha256: digest(supplement.response_id), counts}));
    });
  }),
];
