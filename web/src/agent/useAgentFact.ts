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
    const lifetime = new AbortController();
    let pending = false;
    const load = async () => {
      if (pending || lifetime.signal.aborted || document.visibilityState === 'hidden') return;
      pending = true;
      try {
        const value = await request.read.call(request.client, request.agentId, {
          signal: lifetime.signal,
        });
        if (value.agent_id !== request.agentId) throw new Error('Agent fact identity mismatch');
        if (!lifetime.signal.aborted)
          setState({ request, result: { status: 'ready', agentId: request.agentId, value } });
      } catch {
        if (!lifetime.signal.aborted)
          setState({ request, result: { status: 'unavailable', agentId: request.agentId } });
      } finally {
        pending = false;
      }
    };
    void load();
    const timer = setInterval(() => void load(), 60_000);
    const foreground = () => void load();
    document.addEventListener('visibilitychange', foreground);
    return () => {
      lifetime.abort();
      clearInterval(timer);
      document.removeEventListener('visibilitychange', foreground);
    };
  }, [request]);
  return state?.request === request && enabled ? state.result : { status: 'loading', agentId };
}
