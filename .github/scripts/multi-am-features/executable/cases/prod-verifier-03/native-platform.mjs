import {define, acquire, invoke, egRequest, segment, digest} from './helpers.mjs';
import {send} from './platform.mjs';
import {nativeTelegramCase} from './native-telegram.mjs';
import {nativeWhatsappCase} from './native-whatsapp.mjs';
import {nativeDiscordCase} from './native-discord.mjs';

export const nativePlatformCases = [
  nativeTelegramCase,
  nativeDiscordCase,
  nativeWhatsappCase,
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
