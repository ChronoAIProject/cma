import {define, acquire, invoke, egRequest, peer, segment, same, digest, pages} from './helpers.mjs';

const pathFor = (event, suffix) => `/v1/events/${segment(event.eventRef)}/${suffix}`;
const request = (ctx, lease, options) => egRequest(ctx, lease, {actor: 'agent', ...options});
const counters = lease => invoke(lease, 'counters');

export const toolCases = [
  define(60, 'eg', ['eg'], async ctx => {
    let lease, event, context, before;
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, 'event_context');
      event = await invoke(lease, 'platformSend', {platform: 'custom', text: 'context witness', replica: ctx.am});
      await invoke(lease, 'drain', {timeoutMs: 90000}); before = await counters(lease);
      context = await request(ctx, lease, {path: pathFor(event, 'context')});
      ctx.assert(context.event_id === event.eventId && context.conversation_id === event.conversationId
        && context.channel_id === lease.channelId && context.provider_binding_id === event.bindingId,
      'event_context_exact_target');
      ctx.assert(context.consumer?.id === event.consumerId && context.activity_verified === true,
        'event_context_consumer_attestation');
      ctx.assert(digest(context.activity) === event.activityDigest, 'event_context_activity');
    });
    await ctx.step('step_2', async () => {
      const body = {event_ref: event.eventRef, provider_binding_id: event.bindingId,
        channel_id: lease.channelId, conversation_id: event.conversationId, event_id: event.eventId,
        activity_sha256: event.activityDigest, event_expires_at: context.expires_at,
        binding_generation: event.bindingGeneration};
      for (const replica of [ctx.am, peer(ctx.am)]) {
        const valid = await request(ctx, lease, {method: 'POST', path: '/v1/event-attestations', body, replica});
        ctx.assert(same(valid, {current: true}), 'attestation_boolean_only');
        const wrong = await request(ctx, lease, {method: 'POST', path: '/v1/event-attestations',
          body: {...body, event_id: event.otherEventId}, replica});
        ctx.assert(same(wrong, {current: false}), 'attestation_wrong_event_false');
      }
      const refused = await request(ctx, lease, {path: pathFor(event, 'context'), actor: 'wrong_consumer',
        allowed: [403, 404]});
      ctx.assert(!refused?.activity && !refused?.event_id, 'wrong_consumer_no_context');
    });
    await ctx.step('expected_result', async () => {
      const after = await counters(lease);
      for (const key of ['turns', 'effects', 'executionLeaseRenewals']) {
        ctx.assert(Number.isSafeInteger(before[key]) && before[key] === after[key], 'context_no_effect_' + key.toLowerCase());
      }
      await ctx.observe('event_context_receipt', async () => ({activity_sha256: digest(context.activity),
        event_sha256: digest(context.event_id), before, after}));
    });
  }),
  define(61, 'eg', ['eg'], async ctx => {
    let lease, event, first, target, initial;
    const body = {text: 'synthetic durable reply'}, followup = {text: 'synthetic bounded followup'};
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, 'event_reply'); event = lease.event; initial = await counters(lease);
      first = await request(ctx, lease, {method: 'POST', path: pathFor(event, 'replies'), body, key: lease.keys.reply});
      ctx.assert(first.delivery?.outcome === 'confirmed', 'event_reply_confirmed');
      const observed = await invoke(lease, 'captureEffects', {key: lease.keys.reply});
      ctx.assert(observed.length === 1 && observed[0].destination === event.destination
        && observed[0].contentDigest === digest(body), 'event_reply_correct_destination_content');
    });
    await ctx.step('step_2', async () => {
      target = await request(ctx, lease, {method: 'POST', path: pathFor(event, 'reply-target'), body: {}});
      ctx.assert(typeof target.reply_target_ref === 'string' && target.expires_at <= event.expiresAt,
        'reply_target_bounded');
      const path = `/v1/reply-targets/${segment(target.reply_target_ref)}/messages`;
      const sent = await request(ctx, lease, {method: 'POST', path, body: followup, key: lease.keys.followup});
      ctx.assert(sent.delivery?.outcome === 'confirmed', 'reply_target_followup_confirmed');
      for (const [p, b, key, receipt] of [[pathFor(event, 'replies'), body, lease.keys.reply, first],
        [path, followup, lease.keys.followup, sent]]) {
        const replay = await request(ctx, lease, {method: 'POST', path: p, body: b, key, replica: peer(ctx.am)});
        ctx.assert(same(replay, receipt), 'reply_replay_original_receipt');
        const changed = await request(ctx, lease, {method: 'POST', path: p,
          body: {text: 'changed frozen content'}, key, allowed: [409], replica: peer(ctx.am)});
        ctx.assert(changed.code || changed.type, 'reply_changed_content_typed_conflict');
      }
      // Lose the transport response after acceptance, then read its exact retained
      // receipt. No second POST is made for this unknown outcome.
      const unknown = await invoke(lease, 'dropReplyResponse', {eventRef: event.eventRef,
        body: {text: 'uncertain synthetic reply'}, key: lease.keys.unknown, replica: ctx.am});
      ctx.assert(unknown.outcome === 'unknown', 'reply_unknown_transport_retained');
      const recovered = await invoke(lease, 'reconcileReply', {key: lease.keys.unknown, replica: peer(ctx.am)});
      ctx.assert(recovered.key === lease.keys.unknown && recovered.delivery.outcome === 'confirmed'
        && recovered.sendAttempts === 1, 'reply_unknown_readback_no_resend');
    });
    await ctx.step('expected_result', async () => {
      const final = await invoke(lease, 'drain', {timeoutMs: 90000});
      ctx.assert(final.pending === 0 && final.unknown === 0, 'reply_terminal_finality');
      const effects = await invoke(lease, 'captureEffects', {});
      ctx.assert(effects.length === initial.effects + 3 && effects.every(e => e.destination === event.destination),
        'reply_only_authorized_destination_once');
      await ctx.observe('reply_effect_hashes', async () => effects.map(e => digest(e)));
    });
  }),
  define(62, 'eg', ['eg'], async ctx => {
    let lease;
    const tested = [];
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, 'typing_capabilities');
      ctx.assert(lease.events.length === 2, 'typing_supported_unsupported_fixtures');
      for (const event of lease.events) {
        const context = await request(ctx, lease, {path: pathFor(event, 'context')});
        ctx.assert(context.event_id === event.eventId && context.capabilities.typing === event.supported,
          'typing_exact_capability');
        tested.push({event, capability: context.capabilities.typing});
      }
    });
    await ctx.step('step_2', async () => {
      for (const {event, capability} of tested) {
        const receipt = await request(ctx, lease, {method: 'POST', path: pathFor(event, 'typing')});
        ctx.assert(receipt.outcome === (capability ? 'sent' : 'unsupported'), 'typing_explicit_outcome');
        const effects = await invoke(lease, 'captureEffects', {kind: 'typing', eventId: event.eventId});
        ctx.assert(effects.length === (capability ? 1 : 0), 'typing_capability_effect_bound');
        if (capability) ctx.assert(effects[0].destination === event.destination, 'typing_exact_destination');
      }
    });
    await ctx.step('expected_result', async () => {
      const actual = await counters(lease);
      ctx.assert(actual.turns === lease.baseline.turns && actual.deliveries === lease.baseline.deliveries,
        'typing_no_response_or_message');
      await ctx.observe('typing_counts', async () => actual);
    });
  }),
  define(65, 'eg', ['eg'], async ctx => {
    let lease, event;
    const slots = [];
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, 'media_continuations');
      event = await invoke(lease, 'platformSend', {platform: 'telegram', variant: 'multiple_attachments',
        replica: ctx.am, text: 'synthetic media processing'});
      ctx.assert(event.attachments.length >= 2, 'media_independent_slots_fixture');
      const final = await invoke(lease, 'drain', {eventIds: [event.eventId], timeoutMs: 90000});
      ctx.assert(final.pending === 0 && final.unknown === 0, 'media_processing_terminal');
    });
    await ctx.step('step_2', async () => {
      let cursor;
      const seen = new Set();
      for (let n = 0; n < 20; n++) {
        const result = await egRequest(ctx, lease, {replica: n % 2 ? peer(ctx.am) : ctx.am,
          path: `/v1/channels/${segment(lease.channelId)}/events/${segment(event.eventId)}/media-responses`
            + (cursor == null ? '' : `?cursor=${segment(cursor)}`)});
        ctx.assert(Array.isArray(result.items), 'media_slots_page');
        for (const slot of result.items) {
          const identity = `${slot.consumer_id}:${slot.attachment_index}:${slot.kind}`;
          ctx.assert(!seen.has(identity), 'media_slot_unique'); seen.add(identity); slots.push(slot);
          ctx.assert(slot.consumer_id === event.consumerId && event.attachments.some(a => a.index === slot.attachment_index),
            'media_slot_exact_attachment_consumer');
          ctx.assert(!Object.keys(slot).some(k => /text|path|credential|token/.test(k)), 'media_metadata_no_payload');
        }
        if (result.next_cursor == null) break;
        ctx.assert(result.next_cursor !== cursor && n < 19, 'media_cursor_progress'); cursor = result.next_cursor;
      }
      const expected = await invoke(lease, 'continuationReceipts', {eventId: event.eventId});
      ctx.assert(slots.length === expected.length && slots.length >= 2, 'media_all_slots_returned');
      for (const slot of slots) {
        const receipt = expected.find(e => e.attachmentIndex === slot.attachment_index && e.kind === slot.kind);
        ctx.assert(receipt && slot.response_id === receipt.responseId && slot.status === receipt.status
          && slot.delivery === receipt.delivery, 'media_response_delivery_join');
      }
    });
    await ctx.step('expected_result', async () => {
      ctx.assert(new Set(slots.map(s => s.response_id)).size === slots.length
        && slots.every(s => s.response_id !== event.responseId), 'media_distinct_responses');
      await ctx.observe('media_slot_hashes', async () => slots.map(slot => digest(slot)));
    });
  }),
  define(73, 'eg', ['eg'], async ctx => {
    let lease, capabilities;
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, 'github_inventory');
      capabilities = await egRequest(ctx, lease, {path: '/v1/github/capabilities'});
      for (const key of ['store_ready', 'dynamic_app_registration', 'native_app_webhook',
        'webhook_secret_rotation', 'app_identity_verification', 'ownership_verification', 'automatic_reconciliation']) {
        ctx.assert(typeof capabilities[key] === 'boolean' && capabilities[key] === lease.expected.capabilities[key],
          'github_capability_' + key);
      }
    });
    await ctx.step('step_2', async () => {
      for (const replica of [ctx.am, peer(ctx.am)]) {
        const apps = await pages(ctx, lease, '/v1/github/apps?limit=1', 'app_ref', {replica});
        ctx.assert(apps.length >= 2 && same(apps.map(a => a.app_ref).sort(), [...lease.expected.appRefs].sort()),
          'github_owner_inventory_paginated');
        for (const app of apps) {
          const detail = await egRequest(ctx, lease, {path: `/v1/github/apps/${segment(app.app_ref)}`, replica});
          ctx.assert(detail.app_ref === app.app_ref && detail.version === app.version
            && detail.status === app.status, 'github_inventory_detail_revision');
        }
        const foreign = await egRequest(ctx, lease, {path: `/v1/github/apps/${segment(lease.expected.foreignAppRef)}`,
          replica, allowed: [403, 404]});
        ctx.assert(!foreign.app_ref, 'github_foreign_app_hidden');
      }
    });
    await ctx.step('expected_result', async () => {
      const actual = await counters(lease);
      ctx.assert(actual.turns === lease.baseline.turns && actual.effects === lease.baseline.effects,
        'github_inventory_no_execution_claim');
      await ctx.observe('github_capability_values', async () => capabilities);
    });
  }),
  define(77, 'eg', ['eg', 'eg_accounts'], async ctx => {
    let lease, before, rekeyed;
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, 'service_rekey');
      before = await invoke(lease, 'keyMetadata');
      rekeyed = await egRequest(ctx, lease, {method: 'POST', actor: 'cma_service',
        path: `/v1/channels/${segment(lease.channelId)}/rekey`, body: {}});
      ctx.assert(Number.isSafeInteger(rekeyed.kid) && rekeyed.kid !== before.kid, 'service_rekey_changed_key_id');
      ctx.assert(same(Object.keys(rekeyed), ['kid']), 'service_rekey_no_secret');
    });
    await ctx.step('step_2', async () => {
      for (const actor of ['owner', 'admin']) {
        const rejected = await egRequest(ctx, lease, {method: 'POST', actor,
          replica: peer(ctx.am), path: `/v1/channels/${segment(lease.channelId)}/rekey`, body: {}, allowed: [403]});
        ctx.assert(!('kid' in rejected) && (rejected.code || rejected.type), 'creator_rekey_refused');
      }
    });
    await ctx.step('expected_result', async () => {
      const after = await invoke(lease, 'keyMetadata');
      ctx.assert(after.kid === rekeyed.kid && after.rotationCount === before.rotationCount + 1,
        'service_only_one_key_rotation');
      ctx.assert(after.recordId === before.recordId && after.owner === before.owner, 'rekey_preserves_record_owner');
      await ctx.observe('service_rekey_ids', async () => ({before: before.kid, after: after.kid,
        rotations: after.rotationCount - before.rotationCount}));
    });
  }),
  define(78, 'eg', ['eg', 'eg_accounts'], async ctx => {
    let lease;
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, 'organization_isolation');
      for (const replica of [ctx.am, peer(ctx.am)]) {
        const org = await pages(ctx, lease, '/v1/channels', 'record_id', {replica,
          actor: 'organization_admin', ownerScope: lease.expected.organization});
        ctx.assert(same(org.map(c => c.record_id).sort(), [...lease.expected.organizationRecords].sort()),
          'organization_admin_exact_inventory');
      }
    });
    await ctx.step('step_2', async () => {
      const personal = await pages(ctx, lease, '/v1/channels', 'record_id', {actor: 'owner'});
      ctx.assert(same(personal.map(c => c.record_id).sort(), [...lease.expected.personalRecords].sort()),
        'personal_scope_isolated');
      for (const [actor, ownerScope] of [['owner', lease.expected.organization],
        ['organization_admin', lease.expected.unrelatedOrganization],
        ['organization_credential', lease.expected.unrelatedOrganization]]) {
        const denial = await egRequest(ctx, lease, {path: '/v1/channels', actor, ownerScope, allowed: [403]});
        ctx.assert(!denial.items, 'organization_header_no_authority');
      }
      await invoke(ctx.fixture('eg_accounts'), 'revoke', {identity: 'organization_admin',
        organization: lease.expected.organization, fixture: lease.fixtureId});
      for (const replica of [ctx.am, peer(ctx.am)]) {
        const denied = await egRequest(ctx, lease, {path: '/v1/channels', actor: 'organization_admin',
          ownerScope: lease.expected.organization, replica, allowed: [403]});
        ctx.assert(!denied.items, 'organization_revoked_grant_denied');
      }
    });
    await ctx.step('expected_result', async () => {
      const after = await counters(lease);
      ctx.assert(after.effects === lease.baseline.effects, 'owner_selection_has_no_effect');
      await ctx.observe('organization_isolation_counts', async () => after);
    });
  }),
];
