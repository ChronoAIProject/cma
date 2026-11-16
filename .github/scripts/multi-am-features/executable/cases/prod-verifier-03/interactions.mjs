import {define, acquire, invoke, egRequest, peer, segment, same, digest, poll, retainTurn} from './helpers.mjs';

const request = (ctx, lease, options) => egRequest(ctx, lease, {actor: 'agent', ...options});
const eventPath = event => `/v1/events/${segment(event.eventRef)}`;

async function prepare(ctx, lease, event, message, key) {
  const base = eventPath(event) + '/form-preparation';
  const started = await request(ctx, lease, {method: 'POST', path: base, body: message, key});
  ctx.assert(started.preparation_id && started.customer_message === 'not_sent', 'form_preparation_not_delivery');
  const ready = started.status === 'pending' ? await poll(ctx,
    () => request(ctx, lease, {path: base, replica: peer(ctx.am)}), value => value.status !== 'pending',
    'form_readiness_timeout') : started;
  ctx.assert(ready.preparation_id === started.preparation_id && ready.status === 'ready'
    && ready.customer_message === 'not_sent' && ready.preparation_ref, 'form_exact_ready_preparation');
  const effects = await invoke(lease, 'captureEffects', {kind: 'form', eventId: event.eventId});
  ctx.assert(effects.length === 0, 'form_readiness_has_no_customer_message');
  return ready;
}

export const interactionCases = [
  define(67, 'eg', ['eg'], async ctx => {
    let lease, context, created;
    const completed = [];
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, 'thread_lifecycle');
      context = await request(ctx, lease, {path: eventPath(lease.event) + '/context'});
      ctx.assert(context.event_id === lease.event.eventId
        && context.capabilities.thread_lifecycle?.create_from_original_message === true,
      'thread_original_event_capability');
      const list = await request(ctx, lease, {path: eventPath(lease.event) + '/threads?view=active'});
      ctx.assert(list.scope === 'public_children_of_original_parent' && list.truncated === false
        && list.threads.every(thread => thread.parent_id === lease.expected.parentId), 'thread_authorized_parent_list');
      const body = {name: 'Synthetic native thread', auto_archive_duration: 60};
      created = await request(ctx, lease, {method: 'POST', path: eventPath(lease.event) + '/threads',
        body, key: lease.keys.create});
      ctx.assert(created.outcome.status === 'confirmed' && created.thread.parent_id === lease.expected.parentId
        && created.thread.name === body.name, 'thread_created_under_original_parent');
      await invoke(lease, 'retainThread', {threadId: created.thread.thread_id});
      const replay = await request(ctx, lease, {method: 'POST', path: eventPath(lease.event) + '/threads',
        body, key: lease.keys.create, replica: peer(ctx.am)});
      ctx.assert(same(replay, created), 'thread_create_same_intent_receipt');
    });
    await ctx.step('step_2', async () => {
      const offered = context.capabilities.thread_lifecycle.actions;
      ctx.assert(['rename', 'archive', 'reopen', 'join', 'leave'].every(action => offered.includes(action)),
        'thread_fixture_all_supported_actions');
      for (const action of ['rename', 'archive', 'reopen', 'join', 'leave']) {
        const body = {target: {type: 'thread', thread_ref: created.thread.thread_ref}, action,
          ...(action === 'rename' ? {name: 'Renamed synthetic thread'} : {})};
        const result = await request(ctx, lease, {method: 'POST', path: '/v1/thread-operations',
          body, key: lease.keys[action], replica: peer(ctx.am)});
        ctx.assert(result.outcome.status === 'confirmed' && result.thread.thread_id === created.thread.thread_id
          && result.thread.parent_id === lease.expected.parentId, 'thread_lifecycle_exact_thread');
        if (action === 'rename') ctx.assert(result.thread.name === body.name, 'thread_rename_content');
        if (['archive', 'reopen'].includes(action)) ctx.assert(result.thread.archived === (action === 'archive'),
          'thread_archive_reopen_state');
        completed.push(action);
      }
      for (const event of lease.unsupportedEvents) {
        const c = await request(ctx, lease, {path: eventPath(event) + '/context'});
        ctx.assert(!c.capabilities.thread_lifecycle?.create_from_original_message,
          'private_forum_or_general_topic_not_offered');
        const denied = await request(ctx, lease, {method: 'POST', path: eventPath(event) + '/threads',
          body: {name: 'must not create'}, key: event.key, allowed: [200, 403, 409, 422]});
        ctx.assert(denied.outcome?.status === 'unsupported' || denied.code === 'forbidden'
          || denied.status === 403, 'unsupported_thread_creation_refused');
      }
      const foreign = await request(ctx, lease, {method: 'POST', path: '/v1/thread-operations',
        actor: 'wrong_consumer', key: lease.keys.foreign, body: {target: {type: 'thread',
          thread_ref: created.thread.thread_ref}, action: 'archive'}, allowed: [403, 404]});
      ctx.assert(!foreign.thread, 'thread_wrong_consumer_no_authority');
    });
    await ctx.step('expected_result', async () => {
      const effects = await invoke(lease, 'threadEffects');
      ctx.assert(effects.length === completed.length + 1 && effects.every(effect =>
        effect.parentId === lease.expected.parentId && effect.threadId === created.thread.thread_id),
      'thread_effects_single_parent_no_duplicate');
      ctx.assert(lease.unsupportedEvents.length === 3 && new Set(lease.unsupportedEvents.map(e => e.kind)).size === 3,
        'thread_private_forum_general_negatives_exercised');
      await ctx.observe('thread_effects', async () => effects.map(digest));
    });
  }),
  define(68, 'eg', ['eg'], async ctx => {
    let lease, ready, first, baseline;
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, 'native_interactions'); baseline = await invoke(lease, 'counters');
      ready = await prepare(ctx, lease, lease.event, lease.formMessage, lease.keys.prepare);
      const sent = await request(ctx, lease, {method: 'POST', path: eventPath(lease.event) + '/replies',
        body: lease.formMessage, key: lease.keys.send, formPreparationRef: ready.preparation_ref});
      ctx.assert(sent.delivery?.outcome === 'confirmed', 'form_explicit_commit_confirmed');
      const effects = await invoke(lease, 'captureEffects', {kind: 'form', eventId: lease.event.eventId});
      ctx.assert(effects.length === 1 && effects[0].destination === lease.event.destination
        && effects[0].contentDigest === digest(lease.formMessage), 'form_explicit_exact_customer_message');
    });
    await ctx.step('step_2', async () => {
      for (const control of ['button', 'selection', 'form']) {
        const event = await invoke(lease, 'interactionCallback', {control, variant: 'valid', replica: ctx.am});
        const evidence = await invoke(lease, 'waitForTurn', {eventId: event.eventId, timeoutMs: 90000});
        await retainTurn(ctx, event, evidence);
        ctx.assert(event.consumerId === lease.event.consumerId && event.conversationId === lease.event.conversationId
          && event.controlId === lease.expected.controls[control], 'interaction_intended_control_consumer');
        if (control === 'form') first = event;
      }
      const replay = await invoke(lease, 'interactionCallback', {control: 'form', variant: 'replay', replica: peer(ctx.am)});
      ctx.assert(replay.eventId === first.eventId && replay.responseId === first.responseId, 'interaction_replay_original_response');
      for (const variant of ['expired', 'wrong_consumer']) {
        const denied = await invoke(lease, 'interactionCallback', {control: 'form', variant, replica: peer(ctx.am)});
        ctx.assert(['refused', 'rejected', 'ignored'].includes(denied.outcome) && denied.turnCount === 0,
          'interaction_stale_authority_refused');
      }
      await invoke(lease, 'withdrawSourceConsent', {capability: 'forms', expectedVersion: lease.expected.sourceVersion});
      const revoked = await invoke(lease, 'interactionCallback', {control: 'form', variant: 'pending_after_revocation',
        replica: peer(ctx.am)});
      ctx.assert(['refused', 'rejected', 'ignored'].includes(revoked.outcome) && revoked.turnCount === 0,
        'interaction_current_source_consent_required');
    });
    await ctx.step('expected_result', async () => {
      const final = await invoke(lease, 'drain', {timeoutMs: 90000});
      ctx.assert(final.turns === baseline.turns + 3 && final.pending === 0 && final.unknown === 0,
        'interaction_exact_authorized_turns');
      await ctx.observe('interaction_finality', async () => final);
    });
  }),
  define(42, 'eg', ['eg'], async ctx => {
    let lease, ready;
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, 'whatsapp_group_consent');
      const channel = await request(ctx, lease, {actor: 'owner', path: `/v1/channels/${segment(lease.channelId)}`});
      const source = channel.definition.sources.find(source => source.type === 'whatsapp');
      ctx.assert(source.phone_number_id === lease.expected.phoneId && source.forms_enabled === true
        && same(source.group_ids, lease.expected.groupIds), 'whatsapp_group_verified_phone_scope');
      for (const kind of ['text', 'media']) {
        const event = await invoke(lease, 'platformSend', {platform: 'whatsapp', variant: 'allowed_group_' + kind,
          replica: ctx.am});
        const turn = await invoke(lease, 'waitForTurn', {eventId: event.eventId, timeoutMs: 90000});
        await retainTurn(ctx, event, turn);
        ctx.assert(lease.expected.groupIds.includes(event.destination) && event.phoneId === lease.expected.phoneId,
          'whatsapp_group_admitted_destination');
      }
      for (const variant of ['unlisted_group', 'wrong_phone']) {
        const denied = await invoke(lease, 'platformSend', {platform: 'whatsapp', variant, replica: peer(ctx.am)});
        ctx.assert(['refused', 'rejected'].includes(denied.outcome) && denied.turnCount === 0,
          'whatsapp_other_group_phone_refused');
      }
    });
    await ctx.step('step_2', async () => {
      for (const action of ['update', 'delete']) {
        const result = await request(ctx, lease, {method: 'POST', path: `/v1/messages/${segment(lease.message.ref)}/${action}`,
          body: action === 'update' ? {text: 'edited synthetic group reply'} : {}, key: lease.keys[action]});
        ctx.assert(result.delivery?.outcome === 'confirmed', 'whatsapp_retained_message_effect');
        const effects = await invoke(lease, 'captureEffects', {key: lease.keys[action]});
        ctx.assert(effects.length === 1 && effects[0].messageId === lease.message.platformId
          && effects[0].destination === lease.message.destination, 'whatsapp_edit_delete_original_context');
      }
      const first = await prepare(ctx, lease, lease.event, lease.formMessage, lease.keys.prepare);
      const sent = await request(ctx, lease, {method: 'POST', path: eventPath(lease.event) + '/replies',
        body: lease.formMessage, key: lease.keys.send, formPreparationRef: first.preparation_ref});
      ctx.assert(sent.delivery?.outcome === 'confirmed', 'whatsapp_ready_form_explicit_send');
      const sentEffects = await invoke(lease, 'captureEffects', {kind: 'form', eventId: lease.event.eventId});
      ctx.assert(sentEffects.length === 1 && sentEffects[0].destination === lease.event.destination,
        'whatsapp_ready_form_exact_group');
      ready = await prepare(ctx, lease, lease.pendingEvent, lease.formMessage, lease.keys.pendingPrepare);
      await invoke(lease, 'withdrawSourceConsent', {capability: 'forms_and_groups', expectedVersion: lease.expected.sourceVersion});
      const revoked = await request(ctx, lease, {path: eventPath(lease.pendingEvent) + '/form-preparation', replica: peer(ctx.am)});
      ctx.assert(revoked.status === 'revoked' && revoked.customer_message === 'not_sent'
        && revoked.preparation_id === ready.preparation_id, 'whatsapp_pending_form_revoked');
      const denied = await request(ctx, lease, {method: 'POST', path: eventPath(lease.pendingEvent) + '/replies',
        body: lease.formMessage, key: lease.keys.pendingSend, formPreparationRef: ready.preparation_ref, allowed: [403, 409, 410]});
      ctx.assert(!denied.delivery || denied.delivery.outcome !== 'confirmed', 'whatsapp_revoked_form_not_sent');
    });
    await ctx.step('expected_result', async () => {
      const effects = await invoke(lease, 'captureEffects', {kind: 'form', eventId: lease.pendingEvent.eventId});
      ctx.assert(effects.length === 0, 'whatsapp_revoked_pending_form_zero_effect');
      const final = await invoke(lease, 'drain', {timeoutMs: 90000});
      ctx.assert(final.pending === 0 && final.unknown === 0, 'whatsapp_group_finality');
      await ctx.observe('whatsapp_consent_receipt', async () => ({preparation_sha256: digest(ready.preparation_id), counts: final}));
    });
  }),
];
