import {
  modeUnavailable,
  nativeDiscordProviderUnavailable,
  nativeDiscordUnavailable,
  nativeLarkUnavailable,
  nativeWhatsappUnavailable,
  type RelayDiscovery,
} from '../RelayCapabilities';
import { credentialsPatch, object, records, strings, text, without, type Fields } from './model';
import { telegramMemberJoinGroupIds, telegramRelayJoinIdentityReady } from './telegramMemberJoins';

export function canonical(value: unknown): string {
  return JSON.stringify(value, (_key, item: unknown) =>
    item !== null && typeof item === 'object' && !Array.isArray(item)
      ? Object.fromEntries(Object.entries(item).sort(([a], [b]) => a.localeCompare(b)))
      : item,
  );
}
/** Block new unsupported selections, never rewrite saved records or uncertain retries. */
export function validateRelayAuthoring(
  discovery: RelayDiscovery,
  next: Fields,
  previous?: Fields,
  credentials: Fields = {},
) {
  const oldSources = records(previous?.sources);
  if (records(next.sources).filter((source) => source.type === 'native_lark').length > 1)
    throw new Error('A channel can have only one direct Lark / Feishu producer.');
  for (const [index, source] of records(next.sources).entries()) {
    const unchanged = oldSources.some((old) => canonical(old) === canonical(source));
    if (source.type === 'native_lark') {
      if (
        source.conversation_history === true &&
        !unchanged &&
        discovery.authoring?.native_lark_conversation_history_enabled !== true
      )
        throw new Error(
          'New Lark / Feishu history consent is disabled on this gateway. Your draft has been retained.',
        );
      const saved = oldSources.some((old) => old.type === 'native_lark');
      const onlyRemovedHistory = oldSources.some(
        (old) =>
          old.type === 'native_lark' &&
          old.conversation_history === true &&
          source.conversation_history !== true &&
          canonical({ ...old, conversation_history: undefined }) ===
            canonical({ ...source, conversation_history: undefined }),
      );
      const replacement = credentialsPatch(credentials, false)?.native_lark;
      const reason = nativeLarkUnavailable(discovery, !saved);
      if (reason && ((!unchanged && !onlyRemovedHistory) || replacement))
        throw new Error(`${reason} Your draft has been retained.`);
      if (!saved && !replacement) throw new Error('Enter all three Lark / Feishu credentials.');
      const identity = object(source.identity);
      const safeId = (value: unknown) => /^[A-Za-z0-9_-]{1,128}$/.test(text(value));
      if (
        !['lark', 'feishu'].includes(text(identity.region)) ||
        !['app_id', 'tenant_key', 'bot_open_id'].every((key) => safeId(identity[key])) ||
        !text(identity.app_id).startsWith('cli_') ||
        !text(identity.bot_open_id).startsWith('ou_')
      )
        throw new Error(
          'Enter the exact Lark / Feishu region, app ID (cli_), tenant key and bot open ID (ou_).',
        );
      if (source.separate_app !== true)
        throw new Error(
          `/sources/${index}/separate_app: Confirm that this is a separate app with its own webhook.`,
        );
      if (object(source.admission).bot_messages === 'native_mentions')
        throw new Error('Direct Lark / Feishu receives human messages only.');
      if (object(next.reply).thread_contract != null)
        throw new Error('Direct Lark / Feishu does not support native thread following.');
      continue;
    }
    if (source.type === 'whatsapp') {
      const saved = oldSources.some((old) => old.type === 'whatsapp');
      const replacement = credentialsPatch(credentials, false)?.whatsapp;
      const groups = strings(source.group_ids);
      if (
        source.group_ids !== undefined &&
        (!Array.isArray(source.group_ids) ||
          source.group_ids.length !== groups.length ||
          groups.length > 16 ||
          new Set(groups).size !== groups.length ||
          groups.some((id) => !/^[A-Za-z0-9_+/=-]{1,512}$/.test(id)))
      )
        throw new Error('Enter at most 16 unique, exact WhatsApp group IDs.');
      const retained = oldSources.some(
        (old) =>
          old.type === 'whatsapp' &&
          canonical({
            ...old,
            group_ids: undefined,
            forms_enabled: undefined,
            message_events: undefined,
          }) ===
            canonical({
              ...source,
              group_ids: undefined,
              forms_enabled: undefined,
              message_events: undefined,
            }) &&
          (source.forms_enabled !== true || old.forms_enabled === true) &&
          (source.message_events !== true || old.message_events === true) &&
          groups.every((id) => strings(old.group_ids).includes(id)),
      );
      if (
        discovery.authoring?.native_whatsapp_groups_writes_enabled !== true &&
        groups.some(
          (id) =>
            !oldSources.some(
              (old) =>
                old.type === 'whatsapp' &&
                ['app_id', 'waba_id', 'phone_number_id'].every((key) => old[key] === source[key]) &&
                strings(old.group_ids).includes(id),
            ),
        )
      )
        throw new Error(
          'New WhatsApp group opt-ins are disabled on this gateway. Your draft has been retained.',
        );
      if (
        source.forms_enabled === true &&
        !oldSources.some(
          (old) =>
            old.type === 'whatsapp' &&
            old.forms_enabled === true &&
            ['app_id', 'waba_id', 'phone_number_id'].every((key) => old[key] === source[key]),
        ) &&
        (discovery.authoring?.native_whatsapp_forms_enabled !== true ||
          discovery.authoring?.native_whatsapp_flow_writes_enabled !== true)
      ) {
        throw new Error(
          'New WhatsApp form consent is disabled on this gateway. Your draft has been retained.',
        );
      }
      if (
        source.message_events === true &&
        discovery.authoring?.native_whatsapp_message_events_enabled !== true &&
        !oldSources.some(
          (old) =>
            old.type === 'whatsapp' &&
            old.message_events === true &&
            ['app_id', 'waba_id', 'phone_number_id'].every((key) => old[key] === source[key]),
        )
      )
        throw new Error(
          'New WhatsApp incoming message change consent is disabled on this gateway. Your draft has been retained.',
        );
      const reason = nativeWhatsappUnavailable(discovery, !saved);
      if (reason && (!retained || replacement))
        throw new Error(`${reason} Your draft has been retained.`);
      if (!saved && !replacement) throw new Error('Enter all three WhatsApp credentials.');
      if (
        ['app_id', 'waba_id', 'phone_number_id'].some(
          (key) => !/^[1-9][0-9]{0,31}$/.test(text(source[key])),
        )
      )
        throw new Error(
          'Enter the numeric Meta app, WhatsApp Business account and phone number IDs.',
        );
      if (object(next.partition).key === 'thread')
        throw new Error(
          'Direct WhatsApp uses private chats or explicitly allowed groups. Choose a conversation partition.',
        );
      continue;
    }
    if (source.type === 'telegram' || source.type === 'nyxid_relay') {
      const relay = source.type === 'nyxid_relay';
      const memberJoinChats = strings(source.member_join_chat_ids);
      if (memberJoinChats.length > 0) {
        const allowed = telegramMemberJoinGroupIds(source);
        if (
          (relay ? !telegramRelayJoinIdentityReady(source) : source.message_events !== true) ||
          memberJoinChats.length > 16 ||
          new Set(memberJoinChats).size !== memberJoinChats.length ||
          memberJoinChats.some((id) => !allowed.includes(id))
        )
          throw new Error(
            relay
              ? 'Telegram member joins require a bot ID, bound relay routes and exact group IDs from Chat admission.'
              : 'Telegram member joins require message events and exact group IDs from Chat admission.',
          );
        const retained = oldSources.some(
          (old) =>
            old.type === source.type &&
            old.bot_id === source.bot_id &&
            canonical(old.admission) === canonical(source.admission) &&
            (!relay ||
              canonical(without(old, ['member_join_chat_ids'])) ===
                canonical(without(source, ['member_join_chat_ids']))) &&
            memberJoinChats.every((id) => strings(old.member_join_chat_ids).includes(id)),
        );
        const enabled = relay
          ? discovery.authoring?.relay_telegram_member_joins_enabled
          : discovery.authoring?.native_telegram_member_joins_enabled;
        if (enabled !== true && !retained)
          throw new Error('New Telegram member join settings are disabled on this gateway.');
      }
    }
    if (
      source.type === 'telegram' &&
      source.channel_posts === true &&
      source.message_events !== true
    )
      throw new Error('Channel posts require message events to retain original routing.');
    if (
      source.type === 'telegram' &&
      source.message_events === true &&
      !oldSources.some(
        (old) =>
          canonical(without(old, ['topic_lifecycle', 'topic_creation', 'member_join_chat_ids'])) ===
            canonical(
              without(source, ['topic_lifecycle', 'topic_creation', 'member_join_chat_ids']),
            ) ||
          (source.channel_posts !== true &&
            canonical(
              without(old, ['topic_lifecycle', 'topic_creation', 'member_join_chat_ids']),
            ) ===
              canonical(
                without({ ...source, channel_posts: true }, [
                  'topic_lifecycle',
                  'topic_creation',
                  'member_join_chat_ids',
                ]),
              )),
      ) &&
      discovery.authoring?.native_telegram_message_events_enabled !== true
    )
      throw new Error(
        'New or changed Telegram message event settings are disabled on this gateway. Your draft has been retained.',
      );
    if (
      source.type === 'telegram' &&
      source.topic_lifecycle === true &&
      !oldSources.some(
        (old) =>
          (source.message_events !== true || old.message_events === true) &&
          (source.channel_posts !== true || old.channel_posts === true) &&
          canonical(
            without(old, [
              'message_events',
              'channel_posts',
              'topic_creation',
              'member_join_chat_ids',
            ]),
          ) ===
            canonical(
              without(source, [
                'message_events',
                'channel_posts',
                'topic_creation',
                'member_join_chat_ids',
              ]),
            ),
      ) &&
      discovery.authoring?.native_telegram_topic_lifecycle_enabled !== true
    )
      throw new Error(
        'New or changed Telegram topic management is disabled on this gateway. Your draft has been retained.',
      );
    if (
      source.type === 'telegram' &&
      source.topic_creation === true &&
      !oldSources.some(
        (old) =>
          canonical(
            without(old, [
              'topic_lifecycle',
              'message_events',
              'channel_posts',
              'member_join_chat_ids',
            ]),
          ) ===
          canonical(
            without(source, [
              'topic_lifecycle',
              'message_events',
              'channel_posts',
              'member_join_chat_ids',
            ]),
          ),
      ) &&
      discovery.authoring?.native_telegram_topic_creation_enabled !== true
    )
      throw new Error(
        'New or changed Telegram topic creation is disabled on this gateway. Your draft has been retained.',
      );
    if (source.type === 'discord') {
      if (
        typeof source.intents === 'number' &&
        (source.intents & (1024 | 8192)) !== 0 &&
        !unchanged &&
        discovery.authoring?.native_discord_reactions_enabled !== true
      )
        throw new Error(
          'New or changed reaction access is disabled on this gateway. Your draft has been retained.',
        );
      if (
        source.thread_history === true &&
        !unchanged &&
        discovery.authoring?.native_discord_thread_history_enabled !== true
      )
        throw new Error(
          'New or changed thread history access is disabled on this gateway. Your draft has been retained.',
        );
      if (
        source.thread_lifecycle === true &&
        !unchanged &&
        discovery.authoring?.native_discord_thread_lifecycle_enabled !== true
      )
        throw new Error(
          'New or changed thread management access is disabled on this gateway. Your draft has been retained.',
        );
      if (
        source.conversation_history === true &&
        !unchanged &&
        discovery.authoring?.native_discord_conversation_history_enabled !== true
      )
        throw new Error(
          'New or changed conversation history access is disabled on this gateway. Your draft has been retained.',
        );
      const hasSavedNative = oldSources.some((old) => old.type === 'discord');
      const reason =
        nativeDiscordUnavailable(discovery, !hasSavedNative) ??
        (!hasSavedNative
          ? nativeDiscordProviderUnavailable(discovery, text(object(next.provider).slug))
          : undefined);
      if (
        reason &&
        (!unchanged || Boolean(credentialsPatch(credentials, false)?.discord_bot_token))
      )
        throw new Error(
          `${reason} Keep saved Discord settings unchanged. Your draft has been retained.`,
        );
      if (!hasSavedNative && !credentialsPatch(credentials, false)?.discord_bot_token)
        throw new Error('Enter the Discord bot token. Saved token values are never displayed.');
      continue;
    }
    if (source.type !== 'nyxid_relay') continue;
    if (source.lark_message_effects != null && !unchanged) {
      const effects = object(source.lark_message_effects);
      const binding = object(effects.connection);
      if (discovery.authoring?.lark_message_effects_writes_enabled !== true)
        throw new Error(
          'New or changed message effects are disabled on this gateway. Your draft has been retained.',
        );
      if (
        (!effects.delete && !effects.reaction) ||
        ['user_service_id', 'catalog_service_id', 'app_id'].some(
          (field) => !text(binding[field]).trim(),
        )
      )
        throw new Error(
          'Select at least one message effect and confirm its bot API connection and app ID. Your draft has been retained.',
        );
    }
    if (source.lark_resources != null && !unchanged) {
      const binding = object(source.lark_resources);
      if (
        ['user_service_id', 'catalog_service_id', 'app_id'].some(
          (field) => !text(binding[field]).trim(),
        )
      )
        throw new Error(
          'Choose a bot API connection or enter both connection IDs, and confirm the receiving app ID. Your draft has been retained.',
        );
    }
    if (
      source.lark_resources != null &&
      discovery.authoring?.lark_resource_writes_enabled !== true &&
      !unchanged
    )
      throw new Error(
        'New or changed embedded media bindings are disabled on this gateway. Keep the saved source unchanged or remove its media binding. Your draft has been retained.',
      );
    const platform = String(source.platform);
    const mode =
      source.verify_raw_event === true
        ? 'raw_event'
        : source.trust_normalized === true
          ? 'trusted_normalized'
          : ['telegram', 'lark', 'feishu'].includes(platform)
            ? 'native_event'
            : undefined;
    if (!mode && !unchanged)
      throw new Error(`Choose an explicit incoming message verification policy for ${platform}.`);
    if (!mode) continue;
    const reason = modeUnavailable(discovery, platform, mode);
    if (!reason) continue;
    const sourceChanged =
      !unchanged ||
      (mode === 'raw_event' &&
        (canonical(next.sources) !== canonical(previous?.sources) ||
          canonical(next.partition) !== canonical(previous?.partition) ||
          Object.keys(credentialsPatch(credentials, false) ?? {}).some(
            (key) => key !== 'agent_key',
          )));
    if (sourceChanged)
      throw new Error(
        `${reason} Keep the saved source settings unchanged, or ask the gateway operator to enable this mode. Your draft has been retained.`,
      );
  }
  // Exact existing thread contracts remain editable elsewhere when writes are disabled.
  const thread = object(next.reply).thread_contract;
  if (
    thread &&
    discovery.authoring?.thread_contract_writes_enabled === false &&
    canonical(thread) !== canonical(object(previous?.reply).thread_contract)
  ) {
    throw new Error(
      'New or changed thread-following settings are disabled on this gateway. Your draft has been retained.',
    );
  }
}
