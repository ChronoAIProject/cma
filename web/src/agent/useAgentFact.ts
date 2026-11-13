import { observeRead } from './transport/readObserver';
import { retrySchedule } from './transport/readRetry';
import { useEffect, useMemo, useState } from 'react';
import type { ReadOptions } from '../api/client';
import { useCmaClient } from '../api/context';

export type AgentFactState<Value> =
  | { readonly status: 'loading' | 'unavailable'; readonly agentId: string }
  | { readonly status: 'ready'; readonly agentId: string; readonly value: Value };

/** Optional facts never block conversation loading or survive a changed read context. */
export function useAgentFact<Value extends { readonly agent_id: string }>(
  agentId: string,
  enabled: boolean,
  read: (id: string, options?: ReadOptions) => Promise<Value>,
): AgentFactState<Value> {
  const client = useCmaClient();
  const request = useMemo(
    () => ({ agentId, enabled, client, read }),
    [agentId, enabled, client, read],
  );
  const [state, setState] = useState<{
    readonly request: typeof request;
    readonly result: AgentFactState<Value>;
  }>();
  useEffect(() => {
    if (!request.enabled) return;
    let failures = 0;
    const observer = observeRead(
      async (signal) => {
        try {
          const value = await request.read.call(request.client, request.agentId, {
            signal: signal,
          });
          if (value.agent_id !== request.agentId) throw new Error('Agent fact identity mismatch');
          if (!signal.aborted)
            setState({ request, result: { status: 'ready', agentId: request.agentId, value } });
          failures = 0;
          return { next: 60_000, cooldown: 1000 };
        } catch (error) {
          if (!signal.aborted)
            setState({ request, result: { status: 'unavailable', agentId: request.agentId } });
          if (!signal.aborted) return retrySchedule(error, ++failures);
        }
      },
      { immediateForeground: true },
    );
    return () => observer.dispose();
  }, [request]);
  return state?.request === request && enabled ? state.result : { status: 'loading', agentId };
}
