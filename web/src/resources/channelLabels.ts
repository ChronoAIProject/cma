import type { ChannelUsageSummary } from '@chronoai/cma-sdk';

export type ChannelFilter = 'all' | 'active' | 'history' | 'detached' | 'never';
export const channelFilters: readonly { value: ChannelFilter; label: string }[] = [
  { value: 'all', label: 'With or without a chat channel' },
  { value: 'active', label: 'Currently linked to a channel' },
  { value: 'history', label: 'Any channel history' },
  { value: 'detached', label: 'Previously linked, now detached' },
  { value: 'never', label: 'No channel history' },
];
export function channelQuery(filter: ChannelFilter) {
  return {
    channelActive: filter === 'active' ? true : filter === 'detached' ? false : undefined,
    channelHistory:
      filter === 'history' || filter === 'detached' ? true : filter === 'never' ? false : undefined,
  };
}
export function channelLabel(value: ChannelUsageSummary | undefined): string {
  if (value === undefined) return 'Channel activity unavailable';
  return value.active
    ? 'Linked to a channel'
    : value.has_history
      ? 'Previously linked to a channel'
      : 'No channel history';
}
