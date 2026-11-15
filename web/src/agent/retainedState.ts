import { useCallback, useLayoutEffect, useRef, useState, type SetStateAction } from 'react';
import type { CmaWebClient } from '../api/client';
import type { AgentResourceState } from './controllerTypes';
import { readCooldown, rememberReadCooldown } from './readCooldown';
import { agentReadRetryAt } from './controllerError';

/** Bounded, memory-only views belong to one mounted account and API client. */
class AgentViews {
  constructor(
    private readonly client: CmaWebClient,
    private readonly viewer?: string,
  ) {}
  private readonly values = new Map<string, AgentResourceState>();
  get(id: string | undefined): AgentResourceState {
    if (id === undefined) return { status: 'idle' };
    const retryAt = readCooldown(this.client, this.viewer, id);
    return (
      this.values.get(id) ??
      (retryAt > Date.now()
        ? { status: 'error', message: 'Agent is busy. Retrying automatically.', retryAt }
        : { status: 'loading' })
    );
  }
  put(id: string | undefined, value: AgentResourceState) {
    if (id === undefined) return;
    rememberReadCooldown(this.client, this.viewer, id, agentReadRetryAt(value));
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
  const [scope, setScope] = useState(() => ({
    client,
    viewer,
    views: new AgentViews(client, viewer),
  }));
  let views = scope.views;
  if (scope.client !== client || scope.viewer !== viewer) {
    views = new AgentViews(client, viewer);
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
