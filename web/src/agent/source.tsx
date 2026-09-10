import { useEffect, useState } from 'react';
import type { AgentSource } from '@chronoai/cma-sdk';
import { useCmaClient } from '../api/context';

type SourceState =
  | { readonly status: 'loading' | 'unavailable'; readonly agentId: string }
  | { readonly status: 'ready'; readonly agentId: string; readonly value: AgentSource };

export function useAgentSource(agentId: string, enabled: boolean): SourceState {
  const client = useCmaClient();
  const [state, setState] = useState<SourceState>({ status: 'loading', agentId });
  useEffect(() => {
    if (!enabled) return;
    const lifetime = new AbortController();
    let pending = false;
    const load = async () => {
      if (pending || document.visibilityState === 'hidden') return;
      pending = true;
      try {
        const value = await client.readAgentSource(agentId, { signal: lifetime.signal });
        if (value.agent_id !== agentId) throw new Error('Agent source identity mismatch');
        if (!lifetime.signal.aborted) setState({ status: 'ready', agentId, value });
      } catch {
        if (!lifetime.signal.aborted) setState({ status: 'unavailable', agentId });
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
  }, [agentId, client, enabled]);
  return state.agentId === agentId && enabled ? state : { status: 'loading', agentId };
}

export function AgentSourceLabel({ source }: { readonly source: SourceState }) {
  if (source.status !== 'ready')
    return (
      <p className="agent-header__source">
        {source.status === 'loading' ? 'Loading source' : 'Source unavailable'}
      </p>
    );
  if (source.value.origin.type === 'manual')
    return <p className="agent-header__source">Created manually</p>;
  if (source.value.origin.type === 'workspace_profile' && source.value.trigger === undefined)
    return <p className="agent-header__source">Created from Workspace Profile</p>;
  return (
    <p className="agent-header__source">
      From Trigger
      {source.value.trigger === undefined ? (
        ' (unavailable)'
      ) : (
        <>
          {' '}
          <a href={`/t/${encodeURIComponent(source.value.trigger.public_id)}`}>
            {source.value.trigger.title}
          </a>
        </>
      )}
    </p>
  );
}
