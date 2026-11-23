import { classifyChannelLifecycle } from './channelLifecycle';
import { hasChannelAddress, type Channel, type ChannelListItem } from './channelTypes';

export interface ChannelEvidence {
  readonly channel: ChannelListItem;
  readonly address?: Pick<Channel, 'channel_id' | 'endpoints'>;
  readonly unconfirmed: boolean;
  readonly stale?: boolean;
}

function owner(channel: ChannelListItem): unknown {
  const value = channel.owner ?? channel.definition.owner;
  return value && typeof value === 'object' && 'subject' in value ? value.subject : value;
}

export function channelResponseMatches(previous: ChannelListItem, next: ChannelListItem) {
  return (
    next.record_id === previous.record_id &&
    Number.isSafeInteger(next.version) &&
    next.version >= previous.version &&
    (!previous.channel_id || !next.channel_id || previous.channel_id === next.channel_id) &&
    (owner(previous) === undefined || owner(next) === undefined || owner(previous) === owner(next))
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
    unconfirmed: !Number.isSafeInteger(next.version) || next.version < 0,
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
    !stale && hasChannelAddress(evidence.channel) && (kind === 'complete' || kind === 'legacy')
  );
}
