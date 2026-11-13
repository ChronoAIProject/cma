import type { AgentCreationCategory } from '@chronoai/cma-sdk';
import { useEffect, useMemo, useState } from 'react';
import { creationLabels } from '../agent/provenance';
import type { NavigationSandbox } from './navigation';
import { useTreeCreationScope } from './treeCreationScope';

export type CreationFacts = ReadonlyMap<string, AgentCreationCategory | 'unavailable'>;
const emptyFacts: CreationFacts = new Map();
const retryDelay = 60_000;
const shortLabels: Readonly<Record<AgentCreationCategory, string>> = {
  manual: 'Manual',
  trigger: 'Trigger',
  channel_bridge: 'Channel',
  legacy_unknown: 'Unknown origin',
};

function agentIds(items: readonly NavigationSandbox[]): string[] {
  return [
    ...new Set(
      items.flatMap(({ sandbox, workspaces }) =>
        sandbox.status === 'retired'
          ? []
          : workspaces
              .filter((workspace) => workspace.status !== 'removed')
              .flatMap((workspace) =>
                workspace.agents
                  .filter((agent) => agent.status !== 'deleted')
                  .map((agent) => agent.id),
              ),
      ),
    ),
  ].sort();
}

/** Retain only public categories, with at most four optional reads in flight. */
export function useTreeCreation(
  viewerId: string,
  items: readonly NavigationSandbox[],
  enabled: boolean,
): CreationFacts {
  const scope = useTreeCreationScope(viewerId);
  const identity = JSON.stringify(agentIds(items));
  const request = useMemo(() => ({ scope, identity, enabled }), [scope, identity, enabled]);
  const [state, setState] = useState<{
    readonly request: typeof request;
    readonly facts: CreationFacts;
  }>();
  useEffect(() => {
    if (!request.enabled) return;
    const ids = JSON.parse(request.identity) as string[];
    const lifetime = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    let pending = false;
    const poll = async () => {
      if (pending || lifetime.signal.aborted || document.hidden) return;
      pending = true;
      clearTimeout(timer);
      const missing = ids.filter((id) => !scope.facts.has(id));
      setState({
        request,
        facts: new Map(
          ids.flatMap((id) => {
            const category = scope.facts.get(id);
            return category === undefined ? [] : [[id, category] as const];
          }),
        ),
      });
      let next = 0;
      const worker = async () => {
        while (next < missing.length && !lifetime.signal.aborted && !document.hidden) {
          const id = missing[next++];
          if (id === undefined) return;
          let category: AgentCreationCategory | 'unavailable' = 'unavailable';
          try {
            const value = await scope.client.readAgentProvenance(id, { signal: lifetime.signal });
            if (value.agent_id === id) category = value.creation_origin;
          } catch {
            /* A refused or missing fact never supplies a creation category. */
          }
          if (!lifetime.signal.aborted) {
            if (category !== 'unavailable') scope.facts.set(id, category);
            setState((previous) => ({
              request,
              facts: new Map(previous?.request === request ? previous.facts : emptyFacts).set(
                id,
                category,
              ),
            }));
          }
        }
      };
      await Promise.all(Array.from({ length: Math.min(4, missing.length) }, worker));
      pending = false;
      if (!lifetime.signal.aborted && !document.hidden && ids.some((id) => !scope.facts.has(id)))
        timer = setTimeout(() => void poll(), retryDelay);
    };
    const foreground = () => {
      clearTimeout(timer);
      void poll();
    };
    document.addEventListener('visibilitychange', foreground);
    void poll();
    return () => {
      lifetime.abort();
      clearTimeout(timer);
      document.removeEventListener('visibilitychange', foreground);
    };
  }, [request, scope]);
  return enabled && state?.request === request ? state.facts : emptyFacts;
}

export function CreationBadge({
  id,
  facts,
}: {
  readonly id: string;
  readonly facts: CreationFacts;
}) {
  const category = facts.get(id);
  if (category === undefined || category === 'unavailable') return null;
  return (
    <span className="tree-row__origin" title={creationLabels[category]}>
      {shortLabels[category]}
    </span>
  );
}

export function CreationSummary({
  ids,
  facts,
}: {
  readonly ids: readonly string[];
  readonly facts: CreationFacts;
}) {
  if (
    ids.length === 0 ||
    ids.some((id) => facts.get(id) === undefined || facts.get(id) === 'unavailable')
  )
    return null;
  const categories = Object.keys(shortLabels) as AgentCreationCategory[];
  const label = categories
    .flatMap((category) => {
      const count = ids.filter((id) => facts.get(id) === category).length;
      return count === 0 ? [] : [`${String(count)} ${shortLabels[category].toLowerCase()}`];
    })
    .join(', ');
  return (
    <span className="tree-row__origin" title={`Agent creation origins: ${label}`}>
      {label}
    </span>
  );
}
