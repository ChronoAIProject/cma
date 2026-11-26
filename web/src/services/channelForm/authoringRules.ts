import type { RelayDiscovery } from '../RelayCapabilities';
import { canonical } from './relayAuthoring';
import { object, records, text, without, type Fields } from './model';

/** Partition keys in the gateway's order; labels follow the channel form. */
export const partitionKeys = [
  { value: 'conversation_and_sender', label: 'Each sender in each conversation' },
  { value: 'conversation', label: 'Each conversation' },
  { value: 'thread', label: 'Each thread' },
  { value: 'fixed', label: 'One shared conversation' },
  { value: 'event_subject', label: 'Each event subject' },
  { value: 'per_event', label: 'A new conversation for every event' },
] as const;
const customKeys = ['fixed', 'event_subject', 'per_event'];
const chatKeys = ['conversation_and_sender', 'conversation', 'thread'];
const chatSources = ['nyxid_relay', 'telegram', 'whatsapp', 'discord', 'native_lark'];
const legacyRelaySources = ['nyxid_relay', 'telegram', 'whatsapp', 'discord'];
// The write gate checks the legacy partition rules without these native sources.
const gateIgnoredSources = ['native_lark', 'discord', 'whatsapp'];
const customKeyMessage = 'custom sources require fixed, event_subject, or per_event';

export interface AuthoringIssue {
  readonly path: string;
  readonly message: string;
}

const sourceTypes = (sources: unknown) => records(sources).map((source) => text(source.type));
const hasCustom = (types: readonly string[]) =>
  types.some((type) => type === 'custom' || type === 'github');
const hasChat = (types: readonly string[]) => types.some((type) => chatSources.includes(type));
const extendedWrites = (discovery: RelayDiscovery) =>
  discovery.authoring?.dynamic_sources_writes_enabled === true;
// Unknown until the gateway advertises its authoring capabilities.
const extendedWritesOff = (discovery: RelayDiscovery) =>
  discovery.authoring !== undefined && discovery.authoring.dynamic_sources_writes_enabled !== true;

/** The gateway's legacy partition predicate (`PartitionRule::validate_legacy`). */
function legacyPartitionIssue(key: string, types: readonly string[]): AuthoringIssue | undefined {
  const custom = types.includes('custom');
  if (custom && !customKeys.includes(key))
    return { path: '/partition/key', message: customKeyMessage };
  if ((key === 'event_subject' || key === 'per_event') && !custom)
    return { path: '/partition/key', message: 'requires a custom source' };
  if (key === 'thread' && !types.some((type) => legacyRelaySources.includes(type)))
    return { path: '/partition/key', message: 'requires a chat source' };
  return undefined;
}

/** The gateway's extended partition predicate (`PartitionRule::validate_extended`). */
function extendedPartitionIssue(
  key: string,
  customKey: string | undefined,
  types: readonly string[],
): AuthoringIssue | undefined {
  if (customKey !== undefined) {
    if (!chatKeys.includes(key))
      return { path: '/partition/custom_key', message: 'requires a chat partition key' };
    if (!customKeys.includes(customKey))
      return {
        path: '/partition/custom_key',
        message: 'must be fixed, event_subject, or per_event',
      };
  }
  for (const type of types) {
    if (type === 'whatsapp' && key === 'thread')
      return {
        path: '/partition/key',
        message: 'native WhatsApp private chats do not have threads',
      };
    if ((type === 'custom' || type === 'github') && !customKeys.includes(customKey ?? key))
      return { path: '/partition/key', message: customKeyMessage };
    if (chatSources.includes(type) && key === 'per_event')
      return { path: '/partition/key', message: 'chat sources cannot use per_event' };
  }
  return undefined;
}

/**
 * Partition shapes a new write is certain to be refused for. Anything the
 * gateway might still accept is left to the gateway, which stays authoritative.
 */
export function partitionIssue(
  discovery: RelayDiscovery,
  partition: unknown,
  sources: unknown,
): AuthoringIssue | undefined {
  const rule = object(partition);
  const key = text(rule.key);
  const customKey = rule.custom_key === undefined ? undefined : text(rule.custom_key);
  const types = sourceTypes(sources);
  if (extendedWritesOff(discovery)) {
    const gated = types.filter((type) => !gateIgnoredSources.includes(type));
    if (customKey !== undefined)
      return {
        path: '/partition/custom_key',
        message: 'Grouping custom events separately is not enabled on this gateway.',
      };
    const legacy = gated.length ? legacyPartitionIssue(key, gated) : undefined;
    if (legacy) return legacy;
  }
  return legacyPartitionIssue(key, types) || customKey !== undefined
    ? extendedPartitionIssue(key, customKey, types)
    : undefined;
}

/** Why a partition key is not offered for the current producers, if it is not. */
export function partitionKeyUnavailable(
  discovery: RelayDiscovery,
  key: string,
  sources: unknown,
): string | undefined {
  const types = sourceTypes(sources);
  const custom = hasCustom(types);
  const chat = hasChat(types);
  if (custom && chatKeys.includes(key) && !(chat && extendedWrites(discovery)))
    return 'Custom events need one shared conversation, each event subject or every event.';
  if (!custom && (key === 'event_subject' || key === 'per_event'))
    return 'Needs a custom event producer.';
  if (key === 'thread' && !types.some((type) => chatSources.includes(type) && type !== 'whatsapp'))
    return 'Needs a chat producer with threads.';
  return undefined;
}

/** Whether a separate custom-event grouping applies to these producers. */
export function customGroupingApplies(discovery: RelayDiscovery, sources: unknown): boolean {
  const types = sourceTypes(sources);
  return hasCustom(types) && hasChat(types) && extendedWrites(discovery);
}

/** Choose a valid grouping for a new channel after its producers change. */
export function fitPartition(discovery: RelayDiscovery, draft: Fields): Fields {
  const partition = object(draft.partition);
  if (!partitionIssue(discovery, partition, draft.sources)) return draft;
  const types = sourceTypes(draft.sources);
  const key = text(partition.key);
  const base = { ...without(partition, ['custom_key']), version: partition.version ?? 1 };
  const extended = extendedWrites(discovery);
  const candidates: Fields[] = [
    // Keep the owner's chosen grouping when only the separate custom grouping is invalid.
    ...(partition.custom_key !== undefined ? [{ ...base, key }] : []),
    ...(!hasCustom(types)
      ? [{ ...base, key: 'conversation_and_sender' }]
      : !hasChat(types)
        ? [{ ...base, key: 'event_subject' }]
        : [
            ...(extended && chatKeys.includes(key)
              ? [{ ...base, key, custom_key: 'event_subject' }]
              : []),
            ...(extended
              ? [{ ...base, key: 'conversation_and_sender', custom_key: 'event_subject' }]
              : []),
            { ...base, key: 'fixed' },
          ]),
  ];
  const fitted = candidates.find(
    (candidate) => !partitionIssue(discovery, candidate, draft.sources),
  );
  return fitted ? { ...draft, partition: fitted } : draft;
}

const sinkSignature = (sink: Fields) =>
  canonical(
    sink.type === 'result_webhook'
      ? { type: 'result_webhook', url: text(sink.url).trim(), hmac_key_id: text(sink.hmac_key_id) }
      : { type: text(sink.type) },
  );

/** Destination types a channel can name once. */
export const singleUseSinks = ['source', 'sync'];

/** Which other destination already uses a single-use type, as a 1-based number. */
export function sinkTypeUsedBy(sinks: readonly Fields[], index: number, type: string) {
  if (!singleUseSinks.includes(type)) return undefined;
  const other = sinks.findIndex((sink, i) => i !== index && sink.type === type);
  return other < 0 ? undefined : other + 1;
}

/** The first destination type a new destination can use without duplicating another. */
export function nextSink(sinks: readonly Fields[]): Fields {
  const type = singleUseSinks.find((candidate) => !sinks.some((sink) => sink.type === candidate));
  return type ? { type } : { type: 'result_webhook', url: '', hmac_key_id: '' };
}

export function replyIssue(reply: unknown, previous?: unknown): AuthoringIssue | undefined {
  const value = object(reply);
  const sinks = records(value.sinks);
  const old = object(previous);
  // Repeats a saved channel already had are not re-judged; they deliver nothing extra.
  const saved = records(old.sinks).map(sinkSignature);
  const repeatedBefore = (signature: string) =>
    saved.filter((item) => item === signature).length > 1;
  for (let index = 1; index < sinks.length; index++) {
    const signature = sinkSignature(sinks[index]!);
    const duplicate = sinks.findIndex((sink, i) => i < index && sinkSignature(sink) === signature);
    if (duplicate >= 0 && !repeatedBefore(signature))
      return {
        path: `/reply/sinks/${index}`,
        message: `Reply destination ${index + 1} repeats destination ${duplicate + 1}. Remove it or choose a different destination.`,
      };
  }
  // Existing channels may already rely on the gateway's implicit chat reply.
  // Removing a destination or turning on automatic replies requires an explicit choice.
  const retainedAutomatic = old.auto_final === true && saved.length === 0;
  if (value.auto_final === true && sinks.length === 0 && !retainedAutomatic)
    return {
      path: '/reply/sinks',
      message:
        'Add a reply destination or turn off Send final answers automatically. Final answers need somewhere to go.',
    };
  return undefined;
}

/** Custom producers cannot carry an event schema in the v1 gateway contract. */
export function schemaIssue(sources: unknown): AuthoringIssue | undefined {
  const index = records(sources).findIndex(
    (source) => source.type === 'custom' && source.schema != null,
  );
  return index < 0
    ? undefined
    : {
        path: `/sources/${index}/schema`,
        message:
          'Event schema validation is not supported by this gateway. Remove it before saving.',
      };
}

/**
 * Refuse a channel write the gateway is certain to reject, or one that would
 * promise replies it cannot deliver. Unchanged saved settings are not re-judged.
 */
export function validateAuthoringRules(discovery: RelayDiscovery, next: Fields, previous?: Fields) {
  const changed = (key: string) =>
    previous === undefined || canonical(next[key]) !== canonical(previous[key]);
  const issue =
    schemaIssue(next.sources) ??
    (changed('partition') || changed('sources')
      ? partitionIssue(discovery, next.partition, next.sources)
      : undefined) ??
    (changed('reply') ? replyIssue(next.reply, previous?.reply) : undefined);
  if (issue) throw new Error(`${issue.path}: ${issue.message}`);
}
