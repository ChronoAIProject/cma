import { useCallback, useEffect, useRef, useState } from 'react';
import type { UsageOwnerAllocation } from '@chronoai/cma-sdk';
import type { Metric, MetricsClient } from './model';
import { metricsFailure, type MetricsFailure } from './failure';

interface PageState {
  readonly items: readonly UsageOwnerAllocation[];
  readonly total?: number;
  readonly next?: string | null;
  readonly error?: MetricsFailure;
}
const empty: PageState = { items: [] };

/** One bounded read at a time. Only scrolling or an explicit retry reads another page. */
export function useOwnerAllocations(
  api: MetricsClient,
  reportId: string,
  metric: Metric,
  active: boolean,
) {
  const [cache, setCache] = useState<Partial<Record<Metric, PageState>>>({});
  const [pending, setPending] = useState(false);
  const state = cache[metric] ?? empty;
  const request = useRef<AbortController | undefined>(undefined);
  const stateRef = useRef(state);
  useEffect(() => {
    stateRef.current = state;
  }, [state]);
  const load = useCallback(() => {
    const previous = stateRef.current;
    if (
      !active ||
      request.current ||
      previous.next === null ||
      (previous.error?.retryAt ?? 0) > Date.now() ||
      previous.error?.denied ||
      previous.error?.expired
    )
      return;
    const controller = new AbortController();
    request.current = controller;
    setPending(true);
    void api
      .ownerAllocations(reportId, {
        metric,
        limit: 200,
        cursor: previous.next ?? undefined,
        signal: controller.signal,
      })
      .then((page) => {
        if (controller.signal.aborted) return;
        const seen = new Set(previous.items.map((item) => item.subject));
        if (
          (page.next && page.next === previous.next) ||
          page.items.some((item) => seen.has(item.subject))
        )
          throw new Error('Invalid owner allocation continuation');
        setCache((old) => ({
          ...old,
          [metric]: {
            items: [...previous.items, ...page.items],
            total: Number(page.total_count),
            next: page.next,
          },
        }));
      })
      .catch((error: unknown) => {
        if (!controller.signal.aborted) {
          const failure = metricsFailure(error);
          setCache((old) => ({
            ...old,
            [metric]: { ...(failure.denied || failure.expired ? empty : previous), error: failure },
          }));
        }
      })
      .finally(() => {
        if (request.current === controller) {
          request.current = undefined;
          setPending(false);
        }
      });
  }, [api, reportId, metric, active]);
  useEffect(() => {
    // Parent keys this hook by report identity. Resource switches keep cached pages.
    if (stateRef.current.total === undefined && !stateRef.current.error) load();
    return () => {
      request.current?.abort();
      request.current = undefined;
      setPending(false);
    };
  }, [load]);
  return { ...state, pending, loadMore: load, retry: load };
}
