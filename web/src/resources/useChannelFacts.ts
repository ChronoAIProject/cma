import { useMemo } from 'react';
import type { ChannelUsageSummary } from '@chronoai/cma-sdk';
import type { ResourceViewsClient } from './client';
import { useAuthorizedRead } from './useAuthorizedRead';

export type ChannelFacts = ReadonlyMap<string, ChannelUsageSummary | undefined>;
const empty: ChannelFacts = new Map();
const batchBudgetMs = 12_000;
const requestBudgetMs = 3_000;

/** Optional visible-row facts; four concurrent reads share one bounded, expiring view. */
export function useChannelFacts(
  client: ResourceViewsClient | undefined,
  ids: readonly string[],
  enabled = true,
  scope = '',
): ChannelFacts {
  const identity = JSON.stringify({ scope, ids: [...new Set(ids)].sort() });
  const read = useMemo(
    () =>
      client === undefined || !enabled
        ? undefined
        : async (signal: AbortSignal) => {
            const { ids: targets } = JSON.parse(identity) as { ids: string[] };
            const facts = new Map<string, ChannelUsageSummary | undefined>();
            // Finish with partial facts before useAuthorizedRead's 15-second deadline.
            // Monotonic time keeps a clock correction from extending this batch.
            const deadline = performance.now() + batchBudgetMs;
            let next = 0;
            const worker = async () => {
              while (next < targets.length) {
                signal.throwIfAborted();
                const remaining = Math.floor(deadline - performance.now());
                if (remaining <= 0) return;
                const id = targets[next++];
                if (id === undefined) return;
                try {
                  const value = await client.agents.channelUsage(id, {
                    signal,
                    timeoutMs: Math.min(requestBudgetMs, remaining),
                  });
                  signal.throwIfAborted();
                  facts.set(id, value.channel_usage);
                } catch {
                  signal.throwIfAborted();
                  facts.set(id, undefined);
                }
              }
            };
            await Promise.all(Array.from({ length: Math.min(4, targets.length) }, worker));
            return { value: facts };
          },
    [client, identity, enabled],
  );
  const current = useAuthorizedRead(read);
  return current.state.status === 'ready' ? current.state.value : empty;
}
