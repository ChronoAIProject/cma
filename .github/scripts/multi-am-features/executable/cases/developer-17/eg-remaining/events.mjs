import {
  define,
  begin,
  button,
  input,
  choose,
  toggle,
  openChannel,
  selectNode,
  uiMutation,
  visible,
  other,
  enc,
  same,
  hash,
  unique,
  requireInput,
  poll,
  listField,
} from './core.mjs';
import { validServingReceipt } from '../../../serving.mjs';
const path = (fx) => `/v1/channels/${enc(fx.channel.channel_id)}`;
const ui = (path) => '/api/event-gateway' + path;
async function inspect(ctx, fx) {
  await openChannel(ctx, fx);
  await selectNode(ctx, fx, `Channel: ${fx.channel.definition.name}`);
}
export async function journal(ctx, fx, filter) {
  const rows = await fx.platform.journal(filter);
  ctx.assert(Array.isArray(rows), 'eg20_raw_platform_journal');
  for (const row of rows) {
    ctx.assert(
      (typeof row.eventId === 'string' || typeof row.deliveryId === 'string') &&
        typeof row.kind === 'string',
      'eg20_journal_identity',
    );
    if (row.kind === 'provider_dispatch')
      requireInput(row.amRequests?.length, 'eg20_dispatch_am_receipts_required');
    if (row.amRequests) {
      for (const receipt of row.amRequests)
        ctx.assert(
          validServingReceipt(receipt) &&
            receipt.scope === 'am' &&
            receipt.owner_proof?.agent_sha256 === hash(row.agentId),
          'eg20_platform_am_attribution',
        );
      await ctx.observe('eg20_am_evidence', async () => ({
        schema: 'multi-am-eg-evidence-v1',
        secrets_redacted: true,
        event_sha256: hash(row.eventId ?? row.deliveryId),
        response_sha256: hash(row),
        requests: row.amRequests,
      }));
    }
  }
  await ctx.observe('eg20_platform_journal', async () => rows);
  return rows;
}
const admission = define(24, [
  async (ctx, s) => {
    await begin(ctx, s, 'admission');
    requireInput(s.fx.authority.admissionSource, 'eg20_admission_platform_seed_required');
    // The source is an actual local platform receiver with verified sender/chat identity.
    s.fx.channel = await s.fx.seedChannel({ source: s.fx.authority.admissionSource });
    await openChannel(ctx, s.fx);
    await selectNode(ctx, s.fx, s.fx.authority.admissionNodeLabel);
    await choose(s.page, 'Admission policy', 'Sender, chat and group rules');
    await choose(s.page, 'Senders policy', 'allowlist');
    await listField(s.page, 'Senders IDs', ['coverage-sender-one', 'coverage-sender-two']);
    await choose(s.page, 'Chats policy', 'allowlist');
    await listField(s.page, 'Chats IDs', ['coverage-chat-one', 'coverage-chat-two']);
    await choose(s.page, 'Group messages', 'mention');
    s.saved = (
      await uiMutation(ctx, s.fx, {
        method: 'PUT',
        path: ui(path(s.fx)),
        action: () => button(s.page, 'Save producer').click(),
      })
    ).body;
    ctx.assert(
      s.saved.definition.sources.some((source) => source.admission?.type === 'scoped'),
      'eg20_scoped_admission_saved',
    );
  },
  async (ctx, s) => {
    await inspect(ctx, s.fx);
    await choose(s.page, 'Group incoming messages by', 'Each sender in each conversation');
    await toggle(s.page, 'Send final answers automatically', true);
    await input(s.page, 'Acknowledgement delay (ms)').fill('2000');
    await uiMutation(ctx, s.fx, {
      method: 'PUT',
      path: ui(path(s.fx)),
      action: () => button(s.page, 'Save channel').click(),
    });
    s.events = [];
    for (const [sender, conversation, mentioned] of [
      ['coverage-sender-one', 'coverage-chat-one', true],
      ['coverage-sender-two', 'coverage-chat-one', true],
      ['coverage-sender-one', 'coverage-chat-two', true],
      ['foreign-sender', 'coverage-chat-one', true],
      ['coverage-sender-one', 'foreign-chat', true],
      ['coverage-sender-one', 'coverage-chat-one', false],
    ])
      s.events.push(
        await s.fx.emit({ sender, conversation, mentioned, text: unique('admission') }),
      );
    s.rows = await poll(
      ctx,
      () => journal(ctx, s.fx, { eventIds: s.events.map((e) => e.eventId) }),
      (rows) =>
        s.events.every((event) =>
          rows.some(
            (r) =>
              r.eventId === event.eventId && ['completed', 'refused', 'ignored'].includes(r.kind),
          ),
        ),
      'eg20_admission_terminal_receipts',
    );
    ctx.assert(
      s.events
        .slice(0, 3)
        .every(
          (e) =>
            s.rows.filter((r) => r.eventId === e.eventId && r.kind === 'provider_dispatch')
              .length === 1,
        ),
      'eg20_admitted_only_once',
    );
    ctx.assert(
      s.events
        .slice(3)
        .every(
          (e) => !s.rows.some((r) => r.eventId === e.eventId && r.kind === 'provider_dispatch'),
        ),
      'eg20_denied_events_never_dispatch',
    );
  },
  async (ctx, s) => {
    const allowed = s.events
      .slice(0, 3)
      .map((e) => s.rows.find((r) => r.eventId === e.eventId && r.kind === 'provider_dispatch'));
    ctx.assert(
      new Set(allowed.map((r) => r.conversationId)).size === 3,
      'eg20_sender_conversation_partitions',
    );
    for (const event of s.events.slice(0, 3)) {
      const delivery = s.rows.filter(
        (r) => r.eventId === event.eventId && r.kind === 'final_delivery',
      );
      ctx.assert(
        delivery.length === 1 && delivery[0].destination === event.destination,
        'eg20_exact_auto_final_destination',
      );
    }
    const current = await s.fx.read();
    ctx.assert(
      current.definition.reply.ack_after_ms === 2000 &&
        current.definition.reply.auto_final === true,
      'eg20_reply_policy_peer_read',
    );
  },
]);
const routing = define(26, [
  async (ctx, s) => {
    await begin(ctx, s, 'routing');
    requireInput(s.fx.authority.additionalConsumer, 'eg20_second_consumer_authority_required');
    const retained = requireInput(
      s.fx.authority.routingChannel,
      'eg20_existing_multiconsumer_fixture_required',
    );
    s.fx.channel = await s.fx.read(retained);
    s.consumerId = s.fx.authority.additionalConsumer.id;
    requireInput(
      s.fx.channel.definition.routing?.consumers.some((c) => c.id === s.consumerId && !c.pending),
      'eg20_actual_additional_consumer_required',
    );
    await inspect(ctx, s.fx);
    await listField(s.page, 'Default consumer IDs', ['default']);
    await input(s.page, 'Maximum consumers per event').fill('1');
    await input(s.page, 'Maximum handoff hops').fill('1');
    await input(s.page, 'Events per consumer per minute').fill('2');
    await input(s.page, 'Events per channel per minute').fill('3');
    await button(s.page, 'Add routing rule').click();
    await input(s.page, 'Rule 1 ID').fill('coverage-match');
    await listField(s.page, 'Rule 1 target IDs', [s.consumerId]);
    await listField(s.page, 'Rule 1 Event types', ['coverage.match']);
    s.saved = await uiMutation(ctx, s.fx, {
      method: 'PUT',
      path: ui(path(s.fx) + '/routing'),
      action: () => button(s.page, 'Save routing').click(),
    });
    ctx.assert(
      Number.isSafeInteger(s.saved.submitted.expected_version) &&
        s.saved.body.version > s.saved.submitted.expected_version,
      'eg20_routing_exact_expected_version',
    );
  },
  async (ctx, s) => {
    s.events = [];
    for (const variant of [
      { type: 'coverage.match' },
      { type: 'coverage.other' },
      { type: 'coverage.match', handoff_hop: 2 },
      { type: 'coverage.match' },
      { type: 'coverage.match' },
    ])
      s.events.push(await s.fx.emit({ ...variant, text: unique('routing') }));
    s.rows = await poll(
      ctx,
      () => journal(ctx, s.fx, { eventIds: s.events.map((e) => e.eventId) }),
      (rows) =>
        s.events.every((e) =>
          rows.some(
            (r) => r.eventId === e.eventId && ['completed', 'refused', 'ignored'].includes(r.kind),
          ),
        ),
      'eg20_routing_terminal_receipts',
    );
    ctx.assert(
      s.rows.some(
        (r) =>
          r.eventId === s.events[0].eventId &&
          r.consumerId === s.consumerId &&
          r.kind === 'provider_dispatch',
      ),
      'eg20_matching_consumer_only',
    );
    ctx.assert(
      s.rows.some(
        (r) =>
          r.eventId === s.events[1].eventId &&
          r.consumerId === 'default' &&
          r.kind === 'provider_dispatch',
      ),
      'eg20_unmatched_default_consumer',
    );
    ctx.assert(
      s.rows.some((r) => r.kind === 'refused' && r.reason.includes('hop')) &&
        s.rows.some((r) => r.kind === 'refused' && r.reason.includes('rate')),
      'eg20_hop_and_rate_limits_observed',
    );
  },
  async (ctx, s) => {
    for (const e of s.events)
      ctx.assert(
        s.rows.filter((r) => r.eventId === e.eventId && r.kind === 'provider_dispatch').length <= 1,
        'eg20_max_targets_no_duplicate_dispatch',
      );
    const before = await journal(ctx, s.fx, { recordId: s.fx.channel.record_id });
    const stale = await s.fx.call({
      method: 'PUT',
      path: path(s.fx) + '/routing',
      body: s.saved.submitted,
      replica: other(ctx.am),
      allowed: [409],
    });
    ctx.assert(stale.status === 409, 'eg20_stale_routing_cas');
    const after = await journal(ctx, s.fx, { recordId: s.fx.channel.record_id });
    ctx.assert(same(after, before), 'eg20_losing_mutation_no_provider_effect');
  },
]);
const activity = define(28, [
  async (ctx, s) => {
    await begin(ctx, s, 'activity');
    s.first = await s.fx.emit({ text: unique('retained-event') });
    await poll(
      ctx,
      async () => (await s.fx.eg({ path: path(s.fx) + '/activity' })).body,
      (page) => page.items?.some((e) => e.event_id === s.first.eventId),
      'eg20_retained_activity_ready',
    );
    await openChannel(ctx, s.fx);
    await button(s.page, 'Activity').click();
    await visible(
      ctx,
      s.page.getByText(s.first.eventId, { exact: true }),
      'eg20_retained_event_visible',
    );
    s.next = await s.fx.emit({ text: unique('live-event'), replica: other(ctx.am) });
    await visible(
      ctx,
      s.page.getByText(s.next.eventId, { exact: true }),
      'eg20_live_event_visible',
    );
    await visible(
      ctx,
      s.page.getByText('Live activity', { exact: true }),
      'eg20_live_stream_connected',
    );
  },
  async (ctx, s) => {
    const page = (await s.fx.eg({ path: path(s.fx) + '/activity' })).body;
    ctx.assert(
      typeof page.cursor === 'string' && page.items.some((e) => e.event_id === s.next.eventId),
      'eg20_real_activity_cursor',
    );
    s.cut = await s.fx.fault('disconnectStream', {
      path: ui(path(s.fx) + '/activity/stream'),
      count: 1,
    });
    await s.cut.restore();
    await s.fx.session.switchEG(other(ctx.am));
    await s.page.reload();
    await openChannel(ctx, s.fx);
    await button(s.page, 'Activity').click();
    await visible(
      ctx,
      s.page.getByText(s.next.eventId, { exact: true }),
      'eg20_reconnect_snapshot_retained',
    );
    const expiry = await s.fx.fault('expireActivityCursor', {
      recordId: s.fx.channel.record_id,
      cursor: page.cursor,
    });
    requireInput(typeof expiry.join === 'function', 'eg20_cursor_expiry_join_required');
    await expiry.join();
    const expired = page.cursor;
    const invalid = await s.fx.fault('requestCursor', {
      path: ui(path(s.fx) + '/activity/stream'),
      cursor: expired,
      count: 1,
    });
    requireInput(typeof invalid.frames === 'function', 'eg20_actual_sse_frame_receipts_required');
    await s.page.reload();
    await openChannel(ctx, s.fx);
    await button(s.page, 'Activity').click();
    const frames = await poll(
      ctx,
      () => invalid.frames(),
      (frames) => frames.some((f) => f.event === 'reset'),
      'eg20_real_cursor_reset_deadline',
    );
    await invalid.restore();
    ctx.assert(
      frames.some((f) => f.event === 'reset' && Array.isArray(f.data.items)),
      'eg20_expired_cursor_reset_observed',
    );
  },
  async (ctx, s) => {
    const ids = await s.page.locator('.channel-activity__event code').allTextContents();
    ctx.assert(
      new Set(ids).size === ids.length && ids.includes(s.next.eventId),
      'eg20_atomic_snapshot_no_duplicates',
    );
    const raw = (await s.fx.eg({ path: path(s.fx) + '/activity', replica: other(ctx.am) })).body;
    ctx.assert(
      raw.items.every((row) => !Object.hasOwn(row, 'transcript')),
      'eg20_no_agent_transcript_in_activity',
    );
  },
]);
async function prepareOutbound(ctx, s, attachments = false) {
  await openChannel(ctx, s.fx);
  await selectNode(ctx, s.fx, s.fx.authority.relayNodeLabel);
  s.message = unique('outbound');
  await input(s.page, 'Message text').fill(s.message);
  if (attachments) {
    const media = requireInput(
      s.fx.authority.publicHttpsMedia,
      'eg20_local_https_media_fixture_required',
    );
    ctx.assert(media.length === 4, 'eg20_four_media_boundary_fixture');
    for (let i = 0; i < 4; i++) {
      await button(s.page, 'Add media attachment').click();
      await choose(
        s.page,
        `Media type ${i + 1}`,
        media[i].kind[0].toUpperCase() + media[i].kind.slice(1),
      );
      await input(s.page, `HTTPS URL ${i + 1}`).fill(media[i].url);
    }
    ctx.assert(
      await button(s.page, 'Add media attachment').isDisabled(),
      'eg20_media_count_bounded',
    );
  }
  s.prepared = await uiMutation(ctx, s.fx, {
    method: 'POST',
    path: ui(path(s.fx) + '/outbound-messages'),
    action: () => button(s.page, 'Review message').click(),
    allowed: [200, 201],
    kind: 'outbound',
  });
  ctx.assert(
    s.prepared.body.status === 'prepared' &&
      s.prepared.key &&
      s.prepared.submitted.message.text === s.message,
    'eg20_outbound_exact_prepared_intent',
  );
  ctx.assert(
    s.prepared.body.route_id === s.prepared.submitted.route_id && !s.prepared.body.delivery,
    'eg20_prepare_not_delivery',
  );
}
const outboundPrepare = define(29, [
  async (ctx, s) => {
    await begin(ctx, s, 'outbound-prepare');
    await prepareOutbound(ctx, s, false);
    await visible(ctx, s.page.getByText(s.message, { exact: true }), 'eg20_prepared_text_review');
  },
  async (ctx, s) => {
    await uiMutation(ctx, s.fx, {
      method: 'DELETE',
      path: ui(path(s.fx) + `/outbound-messages/${enc(s.prepared.body.delivery_id)}`),
      action: () =>
        button(s.page.getByRole('region', { name: 'Send to saved destination' }), 'Cancel').click(),
    });
    await button(s.page, 'New message').click();
    await prepareOutbound(ctx, s, true);
    ctx.assert(
      s.prepared.submitted.message.native_attachments.length === 4,
      'eg20_media_preparation_exact_count',
    );
  },
  async (ctx, s) => {
    const id = s.prepared.body.delivery_id;
    const read = (
      await s.fx.eg({ path: path(s.fx) + `/outbound-messages/${enc(id)}`, replica: other(ctx.am) })
    ).body;
    ctx.assert(
      read.delivery_id === id && read.status === 'prepared' && !read.delivery,
      'eg20_prepared_peer_receipt',
    );
    const rows = await journal(ctx, s.fx, { deliveryId: id });
    ctx.assert(!rows.some((r) => r.kind === 'outbound_send'), 'eg20_prepare_never_sent');
  },
]);
const outboundSend = define(30, [
  async (ctx, s) => {
    await begin(ctx, s, 'outbound-send');
    await prepareOutbound(ctx, s);
    const base = path(s.fx) + `/outbound-messages/${enc(s.prepared.body.delivery_id)}`;
    s.cut = await s.fx.fault('dropAcceptedResponse', {
      method: 'POST',
      path: ui(base + '/send'),
      count: 1,
    });
    await button(s.page, 'Send message').click();
    s.accepted = await s.cut.join();
    ctx.assert(s.accepted.accepted === true, 'eg20_uncertain_send_accepted_once');
    await visible(
      ctx,
      button(s.page, 'Check delivery status'),
      'eg20_unknown_send_readback_action',
    );
  },
  async (ctx, s) => {
    await s.cut.restore();
    await s.fx.session.switchEG(other(ctx.am));
    await button(s.page, 'Check delivery status').click();
    const base = path(s.fx) + `/outbound-messages/${enc(s.prepared.body.delivery_id)}`;
    s.read = (await s.fx.eg({ path: base, replica: other(ctx.am) })).body;
    ctx.assert(
      s.read.delivery_id === s.prepared.body.delivery_id &&
        s.read.status === 'committed' &&
        s.read.delivery?.outcome === 'confirmed',
      'eg20_send_exact_peer_receipt',
    );
    await visible(
      ctx,
      s.page.getByText('The platform accepted the message.', { exact: true }),
      'eg20_send_actual_platform_status',
    );
  },
  async (ctx, s) => {
    const rows = await journal(ctx, s.fx, { deliveryId: s.read.delivery_id });
    ctx.assert(
      rows.filter((r) => r.kind === 'outbound_send').length === 1,
      'eg20_unknown_send_never_replayed',
    );
    await button(s.page, 'New message').click();
    await prepareOutbound(ctx, s);
    const cancelled = await uiMutation(ctx, s.fx, {
      method: 'DELETE',
      path: ui(path(s.fx) + `/outbound-messages/${enc(s.prepared.body.delivery_id)}`),
      action: () =>
        button(s.page.getByRole('region', { name: 'Send to saved destination' }), 'Cancel').click(),
    });
    ctx.assert(
      cancelled.body.status === 'cancelled' && !cancelled.body.delivery,
      'eg20_only_unsent_can_cancel',
    );
    const cancelledRows = await journal(ctx, s.fx, { deliveryId: cancelled.body.delivery_id });
    ctx.assert(
      !cancelledRows.some((r) => r.kind === 'outbound_send'),
      'eg20_cancel_has_no_send_effect',
    );
  },
]);
export const cases = [admission, routing, activity, outboundPrepare, outboundSend];
