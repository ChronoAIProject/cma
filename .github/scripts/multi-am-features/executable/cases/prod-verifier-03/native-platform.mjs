import {define, acquire, invoke, egRequest, peer, segment, digest} from './helpers.mjs';
import {send} from './platform.mjs';

export const nativePlatformCases = [
  define(39, 'eg', ['eg'], async ctx => {
    let lease, webhook;
    const turns = [];
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, 'native_telegram');
      webhook = await invoke(lease, 'platformConfiguration', {platform: 'telegram'});
      ctx.assert(webhook.active === true && webhook.callbackPath === `/v1/telegram/${lease.channelId}`
        && webhook.botId === lease.expected.botId, 'telegram_exact_webhook_activated');
      ctx.assert(webhook.trustMode === 'native' && webhook.secretHeaderConfigured === true,
        'telegram_native_authentication');
    });
    await ctx.step('step_2', async () => {
      for (const conversation of ['private', 'permitted_group']) {
        const turn = await send(ctx, lease, {platform: 'telegram', trustMode: 'native', conversation,
          sender: 'allowed_sender', replica: conversation === 'private' ? ctx.am : peer(ctx.am), text: 'native synthetic reply'});
        ctx.assert(turn.receipt.replyToMessageId === turn.event.platformMessageId, 'telegram_original_message_association');
        turns.push(turn);
      }
      const denied = await invoke(lease, 'platformSend', {platform: 'telegram', trustMode: 'native',
        variant: 'invalid_webhook_secret', replica: ctx.am});
      ctx.assert(denied.outcome === 'rejected' && denied.turnCount === 0, 'telegram_invalid_update_refused');
    });
    await ctx.step('expected_result', async () => {
      const final = await invoke(lease, 'drain', {timeoutMs: 90000});
      ctx.assert(final.turns === 2 && final.deliveries === 2 && final.pending === 0 && final.unknown === 0,
        'telegram_only_admitted_native_events');
      ctx.assert(turns[0].event.conversationId !== turns[1].event.conversationId, 'telegram_private_group_context_isolation');
      await ctx.observe('native_telegram_configuration', async () => ({configuration_sha256: digest(webhook), trust_mode: 'native'}));
    });
  }),
  define(40, 'eg', ['eg'], async ctx => {
    let lease, configuration, retained;
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, 'native_discord');
      configuration = await invoke(lease, 'platformConfiguration', {platform: 'discord'});
      const saved = await egRequest(ctx, lease, {path: `/v1/channels/${segment(lease.channelId)}`});
      ctx.assert(configuration.activated === true && configuration.messageContentIntent === true,
        'discord_activation_message_content_intent');
      ctx.assert(saved.native_discord_activation?.capacity > 0
        && saved.native_discord_activation.selected_records <= saved.native_discord_activation.capacity,
      'discord_activation_within_capacity');
      ctx.assert(configuration.botId === lease.expected.botId && configuration.serverId === lease.expected.serverId,
        'discord_exact_test_bot_server');
    });
    await ctx.step('step_2', async () => {
      for (const conversation of ['private', 'permitted_server']) {
        const turn = await send(ctx, lease, {platform: 'discord', trustMode: 'native', conversation,
          replica: conversation === 'private' ? ctx.am : peer(ctx.am), text: 'synthetic native Discord message'});
        retained = turn.event;
        ctx.assert(turn.event.consumerId === lease.expected.consumerId, 'discord_correct_consumer');
      }
      const reaction = await send(ctx, lease, {platform: 'discord', trustMode: 'native',
        variant: 'permitted_reaction', messageId: retained.platformMessageId, replica: peer(ctx.am)});
      ctx.assert(reaction.event.referencedMessageId === retained.platformMessageId,
        'discord_reaction_retained_message');
      for (const variant of ['not_activated_source', 'native_bot_message', 'unknown_message_reaction']) {
        const denied = await invoke(lease, 'platformSend', {platform: 'discord', trustMode: 'native', variant});
        ctx.assert(['refused', 'rejected', 'ignored'].includes(denied.outcome) && denied.turnCount === 0,
          'discord_unsupported_source_or_reference_refused');
      }
    });
    await ctx.step('expected_result', async () => {
      const final = await invoke(lease, 'drain', {timeoutMs: 90000});
      ctx.assert(final.turns === 3 && final.deliveries === 3 && final.pending === 0 && final.unknown === 0,
        'discord_only_activated_admitted_effects');
      ctx.assert(configuration.nativeBotMessagesSupported === false, 'discord_native_bot_messages_not_advertised');
      await ctx.observe('native_discord_counts', async () => final);
    });
  }),
  define(41, 'eg', ['eg'], async ctx => {
    let lease, configuration, turn;
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, 'native_whatsapp');
      configuration = await invoke(lease, 'platformConfiguration', {platform: 'whatsapp'});
      ctx.assert(configuration.phoneId === lease.expected.phoneId && configuration.accountId === lease.expected.accountId,
        'whatsapp_exact_phone_account');
      const challenge = await invoke(lease, 'webhookChallenge', {platform: 'whatsapp', verification: 'valid'});
      ctx.assert(challenge.status === 200 && challenge.bodyDigest === lease.expected.challengeDigest,
        'whatsapp_verified_challenge');
      const denied = await invoke(lease, 'webhookChallenge', {platform: 'whatsapp', verification: 'invalid'});
      ctx.assert(denied.status === 403, 'whatsapp_invalid_challenge_refused');
    });
    await ctx.step('step_2', async () => {
      turn = await send(ctx, lease, {platform: 'whatsapp', trustMode: 'native', conversation: 'private',
        sender: 'allowed_sender', replica: ctx.am, text: 'synthetic private WhatsApp message'});
      ctx.assert(turn.receipt.phoneId === lease.expected.phoneId && turn.event.accountId === lease.expected.accountId,
        'whatsapp_reply_exact_phone');
      for (const variant of ['invalid_signature', 'wrong_phone', 'wrong_account']) {
        const denied = await invoke(lease, 'platformSend', {platform: 'whatsapp', trustMode: 'native', variant,
          replica: peer(ctx.am)});
        ctx.assert(['rejected', 'refused'].includes(denied.outcome) && denied.turnCount === 0,
          'whatsapp_unverified_account_refused');
      }
    });
    await ctx.step('expected_result', async () => {
      const acknowledgements = await invoke(lease, 'captureEffects', {kind: 'acknowledgement'});
      ctx.assert(acknowledgements.length === lease.expected.ackCount
        && acknowledgements.every(e => e.destination === turn.event.destination), 'whatsapp_ack_policy_destination');
      const final = await invoke(lease, 'drain', {timeoutMs: 90000});
      ctx.assert(final.turns === 1 && final.deliveries === 1 && final.pending === 0 && final.unknown === 0,
        'whatsapp_one_verified_private_turn');
      await ctx.observe('native_whatsapp_receipt', async () => ({delivery_sha256: digest(turn.receipt), counts: final}));
    });
  }),
  define(43, 'eg', ['eg'], async ctx => {
    let lease, capabilities, turn;
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, 'native_lark_resources');
      capabilities = await egRequest(ctx, lease, {path: '/v1/channel-capabilities'});
      ctx.assert(capabilities.relay_authoring.native_lark_intake_enabled === true
        && capabilities.relay_authoring.lark_resource_writes_enabled === true, 'native_lark_intake_resource_capabilities');
      ctx.assert(capabilities.relay_authoring.native_lark_controls_enabled === false
        && capabilities.relay_authoring.native_lark_forms_enabled === false, 'native_lark_disabled_controls_fixture');
      const connections = await egRequest(ctx, lease, {path: '/v1/relay-resource-connections'});
      ctx.assert(connections.items.some(item => item.user_service_id === lease.expected.connectionRef),
        'native_lark_selected_resource_connection');
    });
    await ctx.step('step_2', async () => {
      turn = await send(ctx, lease, {platform: 'lark', trustMode: 'native', replica: ctx.am,
        sender: 'allowed_sender', conversation: 'permitted_group', text: 'synthetic Lark native app turn'});
      for (const kind of ['document_read', 'resource_read', 'message_effect']) {
        const result = await invoke(lease, 'resourceOperation', {kind, connectionRef: lease.expected.connectionRef,
          eventRef: turn.event.eventRef, scope: 'selected'});
        ctx.assert(result.connectionRef === lease.expected.connectionRef && result.eventId === turn.event.eventId
          && result.outcome === 'confirmed', 'native_lark_selected_resource_effect');
        const denied = await invoke(lease, 'resourceOperation', {kind, connectionRef: lease.expected.foreignConnectionRef,
          eventRef: turn.event.eventRef, scope: 'foreign'});
        ctx.assert(denied.outcome === 'refused' && denied.effectCount === 0, 'native_lark_foreign_resource_refused');
      }
      const context = await egRequest(ctx, lease, {actor: 'agent',
        path: `/v1/events/${segment(turn.event.eventRef)}/context`});
      ctx.assert(context.source.type === 'native_lark' && context.source.app_id === lease.expected.appId,
        'native_lark_distinct_from_relay');
      ctx.assert(context.capabilities.forms !== true && context.capabilities.buttons === lease.expected.disabledButtons,
        'native_lark_disabled_form_control_not_offered');
    });
    await ctx.step('expected_result', async () => {
      const effects = await invoke(lease, 'resourceEffects');
      ctx.assert(effects.length === 3 && effects.every(e => e.connectionRef === lease.expected.connectionRef),
        'native_lark_effects_within_selected_connection');
      await ctx.observe('native_lark_resource_effects', async () => effects.map(digest));
    });
  }),
];
