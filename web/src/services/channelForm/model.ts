import type { Channel } from '../ChannelEditor';
import { validateHmacCredentials } from './hmacCredentials';

export type Fields = Record<string, unknown>;
export type EditorSection =
  { kind: 'channel' } | { kind: 'producer'; index: number } | { kind: 'consumer'; id: string };
export const object = (value: unknown): Fields =>
  value !== null && typeof value === 'object' && !Array.isArray(value) ? (value as Fields) : {};
export const text = (value: unknown, fallback = ''): string =>
  typeof value === 'string' ? value : fallback;
export const strings = (value: unknown): string[] =>
  Array.isArray(value) ? value.filter((v): v is string => typeof v === 'string') : [];
export const records = (value: unknown): Fields[] =>
  Array.isArray(value) ? value.map(object) : [];
export function without(value: Fields, keys: readonly string[]): Fields {
  return Object.fromEntries(Object.entries(value).filter(([key]) => !keys.includes(key)));
}
export function optional(value: Fields, key: string, next: unknown): Fields {
  return next === undefined ? without(value, [key]) : { ...value, [key]: next };
}
export function defaultProfile(owner = ''): Fields {
  return {
    schema_version: 1,
    metadata: {
      name: 'Channel agent',
      description: '',
      owner: { kind: 'human', subject: owner },
      provider: 'cma_codex',
    },
    model: {
      access: 'managed',
      service: 'chrono-llm-public',
      api_type: 'responses',
      model: 'gpt-6',
      thinking_effort: 'high',
    },
    permission: { action_mode: 'autonomous', service_scope: ['chrono-llm-public', 'ornn-api'] },
    context: { skills: [], system_prompt: '', mcp_servers: [] },
  };
}
export function defaultSource(type: string): Fields {
  if (type === 'native_lark')
    return {
      type,
      separate_app: false,
      identity: { region: 'lark', app_id: '', tenant_key: '', bot_open_id: '' },
      admission: { type: 'allowlist', sender_ids: [] },
    };
  if (type === 'whatsapp')
    return { type, app_id: '', waba_id: '', phone_number_id: '', admission: { type: 'all' } };
  if (type === 'discord')
    return {
      type,
      bot_id: '',
      intents: 37377,
      admission: {
        type: 'scoped',
        senders: { type: 'open' },
        chats: { type: 'open' },
        groups: 'mention',
      },
    };
  if (type === 'telegram') return { type, bot_id: '', admission: { type: 'all' } };
  if (type === 'nyxid_relay')
    return {
      type,
      issuer: '',
      key_id: '',
      route_ids: [''],
      platform: 'telegram',
      admission: { type: 'all' },
    };
  return { type: 'custom', auth: { type: 'hmac', key_id: '' } };
}
/** Retain every writable field, including fields introduced by newer servers.
 * Server-owned identities, digests, receipts and secrets never become write input. */
export function channelDraft(channel?: Channel, owner = ''): Fields {
  if (!channel)
    return {
      schema_version: 1,
      name: '',
      profile: defaultProfile(owner),
      provider: { slug: 'cma_codex' },
      sources: [defaultSource('nyxid_relay')],
      partition: { version: 1, key: 'conversation_and_sender' },
      reply: { auto_final: true, ack_after_ms: 1500, sinks: [{ type: 'source' }] },
    };
  const d = channel.definition;
  return {
    ...without(d, [
      'record_id',
      'creation',
      'owner',
      'profile_digest',
      'secrets',
      'created_at',
      'version',
      'routing',
      'durable_custom_execution',
    ]),
    provider: {
      ...without(object(d.provider), ['binding_id', 'level']),
      slug: channel.provider.slug,
    },
    sources: d.sources.map((source) => without(object(source), ['jwks_url'])),
  };
}
/** A Custom producer verifies events with the shared HMAC list only under HMAC auth. */
export const customUsesHmac = (source: Fields): boolean =>
  source.type === 'custom' && object(source.auth).type === 'hmac';
/** Private drafts stay local until the matching producer is saved. */
export function producerCredentialKeys(source: Fields): readonly string[] {
  switch (source.type) {
    case 'native_lark':
      return ['native_lark'];
    case 'whatsapp':
      return ['whatsapp'];
    case 'discord':
      return ['discord_bot_token'];
    case 'telegram':
      return ['telegram_bot_token', 'telegram_webhook_secret'];
    case 'nyxid_relay':
      return ['channel_key'];
    case 'custom':
      return customUsesHmac(source) ? ['replace_hmac', 'hmac_secrets'] : [];
    default:
      return [];
  }
}
export function usesHmacCredentials(draft: Fields): boolean {
  return (
    (!draft.connection_ref && records(draft.sources).some(customUsesHmac)) ||
    records(object(draft.reply).sinks).some((sink) => sink.type === 'result_webhook')
  );
}
export function channelCredentials(values: Fields, draft: Fields, create: boolean): Fields {
  const keys = new Set([
    'agent_key',
    ...(!draft.connection_ref ? records(draft.sources).flatMap(producerCredentialKeys) : []),
    // The central channel editor retains explicit management of the shared HMAC list.
    ...(!create || usesHmacCredentials(draft) ? ['replace_hmac', 'hmac_secrets'] : []),
  ]);
  return Object.fromEntries(Object.entries(values).filter(([key]) => keys.has(key)));
}
export function credentialsPatch(values: Fields, create: boolean): Fields | undefined {
  const patch: Fields = {};
  const nativeLark = object(values.native_lark);
  const nativeKeys = ['app_secret', 'encrypt_key', 'verification_token'];
  if (nativeKeys.some((key) => text(nativeLark[key]) !== '')) {
    if (nativeKeys.some((key) => text(nativeLark[key]) === ''))
      throw new Error('Enter all three Lark / Feishu credentials to replace the saved bundle.');
    if (nativeKeys.some((key) => !/^[!-~]{16,512}$/.test(text(nativeLark[key]))))
      throw new Error(
        'Lark / Feishu credentials must each contain 16–512 printable ASCII characters without spaces.',
      );
    patch.native_lark = Object.fromEntries(nativeKeys.map((key) => [key, text(nativeLark[key])]));
  }
  const whatsapp = object(values.whatsapp);
  const bundle = ['app_secret', 'access_token', 'verify_token'];
  if (bundle.some((key) => text(whatsapp[key]) !== '')) {
    if (bundle.some((key) => text(whatsapp[key]) === ''))
      throw new Error('Enter all three WhatsApp credentials to replace the saved bundle.');
    patch.whatsapp = Object.fromEntries(bundle.map((key) => [key, text(whatsapp[key])]));
  }
  for (const key of [
    'agent_key',
    'channel_key',
    'telegram_bot_token',
    'telegram_webhook_secret',
    'discord_bot_token',
  ]) {
    const value = text(values[key]);
    if (value !== '') patch[key] = value;
  }
  if (values.replace_hmac === true || (create && values.hmac_secrets !== undefined))
    patch.hmac_secrets = records(values.hmac_secrets).map((row) => ({
      key_id: text(row.key_id),
      secret: text(row.secret),
    }));
  if (create && !patch.agent_key) throw new Error('Enter the provider agent key.');
  return create || Object.keys(patch).length ? patch : undefined;
}
export function channelRequest(
  draft: Fields,
  credentials: Fields,
  channel?: Channel,
  record?: string,
): Fields {
  if (!channel && draft.connection_ref !== undefined && !text(draft.connection_ref))
    throw new Error('Choose a ready CMA-managed Telegram connection.');
  const selected = channelCredentials(credentials, draft, !channel);
  validateHmacCredentials(draft, selected, !channel);
  const patch = credentialsPatch(selected, !channel);
  const provider = object(draft.provider);
  const target = object(provider.target).type;
  const profile = object(draft.profile);
  const usesDefaultEnvironment =
    !channel &&
    provider.slug === 'cma_codex' &&
    (target === undefined || target === 'profile') &&
    profile.environment == null;
  return {
    ...without(draft, ['durable_custom_execution']),
    ...(usesDefaultEnvironment
      ? {
          profile: {
            ...profile,
            environment: {
              size: { cpu_cores: 1, memory_gib: 2 },
              workspace: { type: 'directory', name: 'workspace' },
            },
          },
        }
      : {}),
    ...(patch ? { credentials: patch } : {}),
    ...(channel ? { expected_version: channel.version } : { record_id: record }),
  };
}
const channelKeys = ['name', 'connection_ref', 'connection_admission', 'partition', 'reply'];
export function selectedDraft(base: Fields, draft: Fields, section: EditorSection): Fields {
  if (section.kind === 'producer') {
    const sources = Array.isArray(base.sources) ? [...base.sources] : [];
    const source = Array.isArray(draft.sources) ? draft.sources[section.index] : undefined;
    if (source !== undefined) sources[section.index] = source;
    return { ...base, sources };
  }
  const keys = section.kind === 'channel' ? channelKeys : ['profile', 'provider'];
  let selected = { ...base };
  for (const key of keys) selected = optional(selected, key, draft[key]);
  return selected;
}
/** Adopt confirmed server values for the saved node, retain other node drafts. */
export function confirmedDraft(
  draft: Fields,
  confirmed: Fields,
  section: EditorSection,
  submitted: Fields = {},
): Fields {
  const selected = selectedDraft(draft, confirmed, section);
  if (
    section.kind !== 'channel' ||
    !submitted.connection_ref ||
    submitted.connection_admission === undefined
  )
    return selected;
  // A connection-level admission override changes the saved producer too.
  // Adopt that field without discarding its other unsaved consent choices.
  return {
    ...selected,
    sources: records(selected.sources).map((source) => {
      const saved = records(confirmed.sources).find(
        (value) => value.type === 'telegram' && value.bot_id === source.bot_id,
      );
      return saved ? { ...source, admission: saved.admission } : source;
    }),
  };
}
