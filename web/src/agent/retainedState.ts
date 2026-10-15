import { useCallback, useLayoutEffect, useRef, useState, type SetStateAction } from 'react';
import type { CmaWebClient } from '../api/client';
import type { AgentResourceState } from './controllerTypes';

/** Bounded, memory-only views belong to one mounted account and API client. */
class AgentViews {
  private readonly values = new Map<string, AgentResourceState>();
  get(id: string | undefined): AgentResourceState {
    return id === undefined ? { status: 'idle' } : (this.values.get(id) ?? { status: 'loading' });
  }
  put(id: string | undefined, value: AgentResourceState) {
    if (id === undefined) return;
    this.values.delete(id);
    if (value.status !== 'ready' || value.detail.id !== id) return;
    this.values.set(id, value);
    if (this.values.size > 6) this.values.delete(this.values.keys().next().value!);
  }
}

export function useRetainedAgentState(
  client: CmaWebClient,
  viewer: string | undefined,
  agent: string | undefined,
) {
  const [scope, setScope] = useState(() => ({ client, viewer, views: new AgentViews() }));
  let views = scope.views;
  if (scope.client !== client || scope.viewer !== viewer) {
    views = new AgentViews();
    setScope({ client, viewer, views });
  }
  const [rendered, render] = useState(() => ({ views, agent, value: views.get(agent) }));
  const state =
    rendered.views === views && rendered.agent === agent ? rendered.value : views.get(agent);
  const current = useRef(state);
  const active = useRef({ views, agent });
  useLayoutEffect(() => {
    active.current = { views, agent };
    current.current = state;
  }, [views, agent, state]);
  const setState = useCallback(
    (update: SetStateAction<AgentResourceState>) => {
      if (active.current.views !== views || active.current.agent !== agent) return;
      const next = typeof update === 'function' ? update(current.current) : update;
      current.current = next;
      views.put(agent, next);
      render({ views, agent, value: next });
    },
    [views, agent],
  );
  return [state, setState, current] as const;
}
