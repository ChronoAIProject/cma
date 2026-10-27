import type { Channel } from './ChannelEditor';

export type ChannelSelection =
  | { kind: 'channel' }
  | { kind: 'toolsets' }
  | { kind: 'producer'; index: number }
  | { kind: 'consumer'; id: string };
export interface ChannelNode {
  key: string;
  selection: ChannelSelection;
  title: string;
  subtitle: string;
  detail: string;
  pending?: boolean;
  identity?: string;
}
export const object = (value: unknown): Record<string, unknown> =>
  value !== null && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};
export const text = (value: unknown, fallback = 'Not configured'): string =>
  typeof value === 'string' && value.length > 0 ? value : fallback;
export const list = (value: unknown): readonly unknown[] => (Array.isArray(value) ? value : []);
export const words = (value: unknown, fallback = 'Not configured'): string =>
  text(value, fallback).replaceAll('_', ' ');
export const selectionKey = (selection: ChannelSelection): string =>
  selection.kind === 'producer'
    ? `producer-${selection.index}`
    : selection.kind === 'consumer'
      ? `consumer-${selection.id}`
      : selection.kind;
export function platformName(value: unknown): string {
  const known: Record<string, string> = {
    telegram: 'Telegram',
    whatsapp: 'WhatsApp',
    discord: 'Discord',
    lark: 'Lark',
    feishu: 'Feishu',
    slack: 'Slack',
  };
  return known[text(value)] ?? text(value, 'Relay');
}
export function routing(channel: Channel): Record<string, unknown> {
  return object(object(channel.definition).routing);
}
export function consumerConfig(channel: Channel, id: string): Record<string, unknown> {
  return id === 'default'
    ? {
        ...object(routing(channel).default_consumer),
        provider: channel.definition.provider,
        profile: channel.definition.profile,
      }
    : object(list(routing(channel).consumers).find((value) => object(value).id === id));
}
export function channelNodes(channel: Channel) {
  const producers: ChannelNode[] = channel.definition.sources.map((value, index) => {
    const source = object(value);
    const relay = source.type === 'nyxid_relay';
    return {
      key: `producer-${index}`,
      selection: { kind: 'producer', index },
      identity: relay ? text(source.platform, '') : text(source.type, ''),
      title:
        source.type === 'custom'
          ? 'Custom events'
          : source.type === 'discord'
            ? 'Discord'
            : source.type === 'telegram'
              ? 'Telegram'
              : source.type === 'whatsapp'
                ? 'WhatsApp'
                : relay
                  ? platformName(source.platform)
                  : `Producer ${index + 1}`,
      subtitle: relay
        ? 'NyxID relay'
        : source.type === 'discord'
          ? 'Direct Discord bot'
          : source.type === 'telegram'
            ? 'Direct webhook'
            : source.type === 'whatsapp'
              ? 'Direct WhatsApp webhook'
              : source.type === 'custom'
                ? 'HTTP events'
                : 'Recorded source',
      detail: relay
        ? list(source.route_ids).length === 0
          ? 'No routes'
          : `${list(source.route_ids).length} ${list(source.route_ids).length === 1 ? 'route' : 'routes'}`
        : source.bot_username
          ? `@${text(source.bot_username).replace(/^@/, '')}`
          : source.type === 'custom'
            ? 'Authenticated ingress'
            : source.type === 'whatsapp'
              ? text(source.phone_number_id, 'Phone number not reported')
              : text(source.bot_id, 'Source configured'),
    };
  });
  const defaults = object(routing(channel).default_consumer);
  const consumers: ChannelNode[] = [
    {
      key: 'consumer-default',
      selection: { kind: 'consumer', id: 'default' },
      title: text(defaults.label, text(defaults.name, 'Default consumer')),
      subtitle: channel.provider.slug,
      detail: 'Default consumer',
      identity: providerIdentity(channel.provider.slug),
    },
    ...list(routing(channel).consumers).map((value): ChannelNode => {
      const consumer = object(value);
      const id = text(consumer.id, 'unknown');
      return {
        key: `consumer-${id}`,
        selection: { kind: 'consumer', id },
        title: text(consumer.label, text(consumer.name, id)),
        subtitle: text(object(consumer.provider).slug),
        identity: providerIdentity(text(object(consumer.provider).slug)),
        detail: consumer.pending === true ? 'Binding pending' : text(consumer.name, id),
        pending: consumer.pending === true,
      };
    }),
  ];
  const toolsets: ChannelNode = {
    key: 'toolsets',
    selection: { kind: 'toolsets' },
    identity: 'toolsets',
    title: 'Agent toolsets',
    subtitle: 'Messaging and event tools',
    detail: 'Explore tools and availability',
  };
  return { producers, consumers, toolsets };
}
export function admissionSummary(value: unknown): string {
  const policy = object(value);
  if (policy.type === 'all') return 'All senders';
  if (policy.type === 'allowlist') return `${list(policy.sender_ids).length} allowed senders`;
  if (policy.type === 'scoped') return 'Restricted by sender, chat and group rules';
  return 'Not recorded';
}

function providerIdentity(slug: string): string | undefined {
  if (['cma', 'cma_codex', 'cma_agent', 'chronoai-managed-agent'].includes(slug)) return 'cma';
  if (slug === 'nyxbot' || slug.startsWith('nyxbot_') || slug.startsWith('nyxbot-'))
    return 'nyxbot';
  return undefined;
}
