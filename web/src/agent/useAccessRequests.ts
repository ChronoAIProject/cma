import { useEffect, useMemo, useRef, useState } from 'react';
import { accessRequests, type AgentAccessRequest } from './accessRequests';
import { observeAccessRequests } from './accessRequestObservation';

export function useAccessRequests(
  agentId: string,
  owner: string | undefined,
  enabled: boolean,
  running: boolean,
  eventRevision: string | number,
) {
  const scope = useMemo(() => ({ agentId, owner }), [agentId, owner]);
  const [state, setState] = useState<{
    scope: typeof scope;
    requests: readonly AgentAccessRequest[];
    error?: string;
  }>();
  const observer = useRef<ReturnType<typeof observeAccessRequests> | undefined>(undefined);
  const observedRevision = useRef(eventRevision);
  const currentRunning = useRef(running);
  useEffect(() => {
    const started = !currentRunning.current && running;
    currentRunning.current = running;
    if (started) observer.current?.refresh();
  }, [running]);
  useEffect(() => {
    if (!enabled) return;
    const next = observeAccessRequests({
      running: () => currentRunning.current,
      read: (signal) => accessRequests.list(scope.agentId, signal),
      publish: (requests) => setState({ scope, requests }),
      failed: (cause) =>
        setState((previous) => ({
          scope,
          requests: previous?.scope === scope ? previous.requests : [],
          error:
            cause instanceof Error ? cause.message : 'Agent access requests could not be checked.',
        })),
    });
    observer.current = next;
    return () => {
      next.dispose();
      observer.current = undefined;
    };
  }, [scope, enabled]);
  useEffect(() => {
    if (observedRevision.current !== eventRevision) {
      observedRevision.current = eventRevision;
      if (running) observer.current?.hint();
    }
  }, [eventRevision, running]);
  return {
    requests: enabled && state?.scope === scope ? state.requests : [],
    error: enabled && state?.scope === scope ? state.error : undefined,
    refresh: () => observer.current?.refresh(),
    update: (value: AgentAccessRequest) =>
      setState((previous) =>
        previous?.scope === scope
          ? {
              ...previous,
              requests: previous.requests.map((request) =>
                request.id === value.id ? value : request,
              ),
            }
          : previous,
      ),
  };
}
