import {define, acquire, invoke, egRequest, peer, segment, same, digest} from './helpers.mjs';

const request = (ctx, lease, options) => egRequest(ctx, lease, {actor: 'agent', ...options});
const eventPath = event => `/v1/events/${segment(event.eventRef)}`;

export const mediaToolCases = [
  define(63, 'eg', ['eg'], async ctx => {
    let lease, message, capabilities;
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, 'message_effects'); message = lease.message;
      capabilities = await request(ctx, lease, {path: `/v1/messages/${segment(message.ref)}/capabilities`});
      ctx.assert(capabilities.delete && capabilities.reaction && capabilities.media_edit?.content,
        'message_effects_fixture_capabilities');
      ctx.assert(capabilities.scope === lease.expected.messageScope, 'message_effect_exact_scope');
    });
    await ctx.step('step_2', async () => {
      const actions = [
        ['text_edit', 'update', {text: 'changed synthetic text'}],
        ['media_edit', 'update', lease.expected.mediaPatch],
        ['reaction', 'reaction', lease.expected.reaction],
        ['delete', 'delete', {}],
      ];
      for (const [name, action, body] of actions) {
        const path = `/v1/messages/${segment(message.ref)}/${action}`, key = lease.keys[name];
        const receipt = await request(ctx, lease, {method: 'POST', path, body, key});
        ctx.assert(receipt.delivery?.outcome === 'confirmed', 'message_effect_confirmed');
        const observed = await invoke(lease, 'captureEffects', {key});
        ctx.assert(observed.length === 1 && observed[0].messageId === message.platformId
          && observed[0].destination === message.destination, 'message_effect_exact_message');
        const stale = await request(ctx, lease, {method: 'POST', path: `/v1/messages/${segment(message.staleRef)}/${action}`,
          body, key: lease.keys['stale_' + name], replica: peer(ctx.am), allowed: [403, 404, 409, 410]});
        ctx.assert(stale.code || stale.type, 'message_stale_reference_refusal');
      }
      for (const action of ['update', 'reaction', 'delete']) {
        const unsupported = await request(ctx, lease, {method: 'POST',
          path: `/v1/messages/${segment(message.unsupportedRef)}/${action}`,
          body: action === 'update' ? {text: 'unsupported'} : action === 'reaction' ? lease.expected.reaction : {},
          key: lease.keys['unsupported_' + action], allowed: [200, 400, 403, 409, 422]});
        ctx.assert(unsupported.delivery?.outcome === 'unsupported' || unsupported.code === 'unsupported',
          'unsupported_message_effect_explicit');
      }
      const uncertain = await invoke(lease, 'dropMessageEffectResponse', {messageRef: message.uncertainRef,
        action: 'reaction', body: lease.expected.reaction, key: lease.keys.unknown});
      ctx.assert(uncertain.outcome === 'unknown', 'message_effect_unknown_retained');
      const recovered = await invoke(lease, 'reconcileMessageEffect', {key: lease.keys.unknown});
      ctx.assert(recovered.key === lease.keys.unknown && recovered.sendAttempts === 1
        && recovered.delivery.outcome === 'confirmed', 'message_effect_unknown_no_replay');
    });
    await ctx.step('expected_result', async () => {
      const effects = await invoke(lease, 'captureEffects', {});
      ctx.assert(effects.length === 5 && effects.every(effect =>
        [message.platformId, message.uncertainPlatformId].includes(effect.messageId)), 'message_effects_only_authorized_messages');
      const final = await invoke(lease, 'drain', {timeoutMs: 90000});
      ctx.assert(final.unknown === 0 && final.pending === 0, 'message_effects_reconciled');
      await ctx.observe('message_effect_receipts', async () => effects.map(digest));
    });
  }),
  define(64, 'eg', ['eg'], async ctx => {
    let lease;
    const events = [];
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, 'inbound_attachments');
      for (const kind of ['image', 'document', 'audio', 'video']) {
        const event = await invoke(lease, 'platformSend', {platform: 'telegram', variant: kind, replica: ctx.am});
        ctx.assert(event.attachments.length === 1 && event.attachments[0].kind === kind,
          'inbound_exact_attachment_kind'); events.push(event);
      }
      await invoke(lease, 'awaitMediaReady', {eventIds: events.map(e => e.eventId), timeoutMs: 90000});
    });
    await ctx.step('step_2', async () => {
      for (const event of events) {
        const attachment = event.attachments[0];
        const path = `${eventPath(event)}/attachments/${attachment.index}`;
        const view = await request(ctx, lease, {path: path + '?view=download', replica: peer(ctx.am)});
        ctx.assert(view.status === 'ready' && view.index === attachment.index
          && view.size_bytes === attachment.size && view.mime_type === attachment.mediaType, 'attachment_exact_ready_metadata');
        ctx.assert(view.size_bytes <= lease.expected.maxBytes && view.download.method === 'GET'
          && new URL(view.download.url).pathname.endsWith('/v1/media/content')
          && new URL(view.download.url).search === '', 'attachment_bounded_download_url');
        // The opaque capability is consumed inside the adapter and never retained
        // by ctx.observe; do not execute the response's curl text.
        const first = await invoke(lease, 'download', {authorization: view.download.authorization, path: '/v1/media/content'});
        ctx.assert(first.status === 200 && first.sha256 === attachment.digest && first.size === attachment.size,
          'attachment_download_exact_frozen_bytes');
        const replay = await invoke(lease, 'download', {authorization: view.download.authorization, path: '/v1/media/content'});
        ctx.assert([401, 403, 404, 410].includes(replay.status) && replay.size === 0, 'attachment_download_single_use');
        for (const suffix of ['/attachments/15?view=download', `/attachments/${attachment.index}?view=text&page=65`]) {
          const refused = await request(ctx, lease, {path: eventPath(event) + suffix, allowed: [400, 403, 404, 422]});
          ctx.assert(!refused.download && !refused.text && !refused.images, 'attachment_index_page_refused');
        }
        const foreign = await request(ctx, lease, {path: path + '?view=download', actor: 'wrong_consumer', allowed: [403, 404]});
        ctx.assert(!foreign.download, 'attachment_foreign_consumer_refused');
      }
    });
    await ctx.step('expected_result', async () => {
      const downloads = await invoke(lease, 'downloadReceipts');
      ctx.assert(downloads.filter(d => d.status === 200).length === 4
        && downloads.filter(d => d.status === 200).every(d => events.some(e => e.attachments[0].digest === d.sha256)),
      'attachments_only_retained_authorized_bytes');
      await ctx.observe('attachment_download_hashes', async () => downloads.map(d => ({status: d.status, sha256: d.sha256, size: d.size})));
    });
  }),
  define(66, 'eg', ['eg'], async ctx => {
    let lease, event;
    const messages = [];
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, 'conversation_history'); event = lease.event;
      await invoke(lease, 'setSourceConsent', {sourceId: event.sourceId, history: true});
      const context = await request(ctx, lease, {path: eventPath(event) + '/context'});
      ctx.assert(context.capabilities.conversation_history != null && context.event_id === event.eventId,
        'history_exact_source_consent');
    });
    await ctx.step('step_2', async () => {
      let cursor;
      const cursors = new Set();
      for (let n = 0; n < 20; n++) {
        const page = await request(ctx, lease, {path: eventPath(event) + '/history'
          + (cursor == null ? '' : `?cursor=${segment(cursor)}`), replica: n % 2 ? peer(ctx.am) : ctx.am});
        ctx.assert(same(page.conversation, lease.expected.conversation)
          && page.anchor_message_id === lease.expected.anchorMessageId, 'history_exact_conversation_anchor');
        ctx.assert(page.messages.length <= lease.expected.pageLimit && page.content_coverage === 'platform_returned_only',
          'history_page_bounded_coverage');
        for (const message of page.messages) {
          ctx.assert(lease.expected.messageIds.includes(message.message_id) && !messages.some(m => m.message_id === message.message_id),
            'history_only_authorized_unique_messages');
          ctx.assert(!JSON.stringify(message).includes('/v1/media/content'), 'history_not_download_authority');
          messages.push(message);
        }
        if (page.continuation.status === 'end_of_visible_history') break;
        ctx.assert(page.continuation.status === 'more_visible_rows' && n < 19
          && !cursors.has(page.continuation.cursor), 'history_cursor_progress');
        cursor = page.continuation.cursor; cursors.add(cursor);
        const wrong = await request(ctx, lease, {path: `/v1/events/${segment(lease.expected.otherEventRef)}/history?cursor=${segment(cursor)}`,
          allowed: [400, 403, 404, 409, 422]});
        ctx.assert(!wrong.messages, 'history_cursor_not_transferable');
      }
      ctx.assert(same(messages.map(m => m.message_id).sort(), [...lease.expected.messageIds].sort()), 'history_visible_retention_complete');
    });
    await ctx.step('expected_result', async () => {
      await invoke(lease, 'setSourceConsent', {sourceId: event.sourceId, history: false});
      const revoked = await request(ctx, lease, {path: eventPath(event) + '/history', allowed: [403, 409, 422]});
      ctx.assert(!revoked.messages, 'history_revoked_consent_refused');
      await ctx.observe('history_message_hashes', async () => messages.map(digest));
    });
  }),
];
