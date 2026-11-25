import { classifyChannelLifecycle } from './channelLifecycle';
import { hasChannelAddress, type Channel, type ChannelListItem } from './channelTypes';

export interface ChannelEvidence {
  readonly channel: ChannelListItem;
  readonly address?: Pick<Channel, 'channel_id' | 'endpoints'>;
  readonly unconfirmed: boolean;
  readonly stale?: boolean;
}

type Owner = { readonly subject: string; readonly kind?: string } | 'absent' | 'invalid';
const ownerKinds = new Set(['human', 'service_account', 'agent_key', 'organization']);
function owner(channel: ChannelListItem): Owner {
  const canonical = Object.hasOwn(channel.definition, 'owner');
  const value = canonical ? channel.definition.owner : channel.owner;
  if (!canonical && value === undefined) return 'absent';
  if (!canonical && typeof value === 'string' && value.length > 0) return { subject: value };
  if (value === null || typeof value !== 'object' || Array.isArray(value)) return 'invalid';
  if (!('subject' in value) || typeof value.subject !== 'string' || value.subject.length === 0)
    return 'invalid';
  if (!canonical && !('kind' in value)) return { subject: value.subject };
  if (!('kind' in value) || typeof value.kind !== 'string' || !ownerKinds.has(value.kind))
    return 'invalid';
  return { kind: value.kind, subject: value.subject };
}
function ownerMatches(previous: ChannelListItem, next: ChannelListItem) {
  const before = owner(previous),
    after = owner(next);
  if (after === 'invalid') return false;
  if (before === 'absent' || before === 'invalid') return true;
  if (after === 'absent') return false;
  return before.subject === after.subject && (!before.kind || before.kind === after.kind);
}

export function channelResponseMatches(previous: ChannelListItem, next: ChannelListItem) {
  return (
    next.record_id === previous.record_id &&
    Number.isSafeInteger(next.version) &&
    next.version >= previous.version &&
    (!previous.channel_id || !next.channel_id || previous.channel_id === next.channel_id) &&
    ownerMatches(previous, next)
  );
}

export function mergeChannelEvidence(
  previous: ChannelEvidence | undefined,
  next: ChannelListItem,
  authoritative = false,
): ChannelEvidence {
  const address = hasChannelAddress(next)
    ? { channel_id: next.channel_id, endpoints: next.endpoints }
    : undefined;
  if (previous) {
    if (next.record_id !== previous.channel.record_id) return { ...previous, unconfirmed: true };
    if (next.version < previous.channel.version) return previous;
    if (
      !channelResponseMatches(previous.channel, next) ||
      (previous.address && next.channel_id && previous.address.channel_id !== next.channel_id)
    )
      return { ...previous, unconfirmed: true };
    const sameVersion = next.version === previous.channel.version;
    const contradiction =
      classifyChannelLifecycle(previous.channel.definition).kind !==
      classifyChannelLifecycle(next.definition).kind;
    if (
      sameVersion &&
      (contradiction || previous.unconfirmed) &&
      !(authoritative && previous.unconfirmed)
    )
      return {
        ...previous,
        address: previous.address ?? address,
        unconfirmed: true,
      };
  }
  return {
    channel: next,
    address: address ?? previous?.address,
    unconfirmed:
      !Number.isSafeInteger(next.version) || next.version < 0 || owner(next) === 'invalid',
  };
}

export function channelEvidenceState(evidence: ChannelEvidence) {
  return evidence.unconfirmed
    ? ({ kind: 'unconfirmed' } as const)
    : classifyChannelLifecycle(evidence.channel.definition);
}

export function channelIsWritable(evidence: ChannelEvidence, stale: boolean) {
  const kind = channelEvidenceState(evidence).kind;
  return (
    !stale &&
    !evidence.stale &&
    hasChannelAddress(evidence.channel) &&
    (kind === 'complete' || kind === 'legacy')
  );
}
