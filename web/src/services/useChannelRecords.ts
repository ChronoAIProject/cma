import { useCallback, useState } from 'react';
import { mergeChannelEvidence, type ChannelEvidence } from './channelEvidence';
import type { Channel, ChannelListItem } from './channelTypes';

type Records = ReadonlyMap<string, ChannelEvidence>;
export function useChannelRecords(
  page: { readonly items: readonly ChannelListItem[] } | undefined,
  created: Channel | undefined,
) {
  const [state, setState] = useState({
    page: undefined as typeof page,
    created: undefined as typeof created,
    records: new Map() as Records,
    refreshEvidence: new Map() as Records,
  });
  let records = state.records;
  if (page !== state.page || created !== state.created) {
    const next = new Map(records);
    if (created && created !== state.created)
      next.set(created.record_id, mergeChannelEvidence(next.get(created.record_id), created));
    if (page !== state.page) {
      const seen = new Set<string>();
      for (const channel of page?.items ?? []) {
        next.set(
          channel.record_id,
          mergeChannelEvidence(
            next.get(channel.record_id),
            channel,
            state.refreshEvidence.get(channel.record_id) === next.get(channel.record_id) &&
              !seen.has(channel.record_id),
          ),
        );
        seen.add(channel.record_id);
      }
    }
    records = next;
    setState({
      page,
      created,
      records,
      refreshEvidence: page !== state.page ? new Map() : state.refreshEvidence,
    });
  }
  const observe = useCallback((channel: ChannelListItem) => {
    setState((previous) => ({
      ...previous,
      records: new Map(previous.records).set(
        channel.record_id,
        mergeChannelEvidence(previous.records.get(channel.record_id), channel),
      ),
    }));
  }, []);
  const unavailable = useCallback((id: string) => {
    setState((previous) => {
      const record = previous.records.get(id);
      return record
        ? {
            ...previous,
            records: new Map(previous.records).set(id, { ...record, stale: true }),
          }
        : previous;
    });
  }, []);
  const beginRefresh = useCallback(() => {
    setState((previous) => {
      const records = new Map(
        [...previous.records].map(([id, value]) => [id, { ...value, stale: true }]),
      );
      return {
        ...previous,
        records,
        refreshEvidence: new Map([...records].filter(([, value]) => value.unconfirmed)),
      };
    });
  }, []);
  return { records, observe, unavailable, beginRefresh };
}
