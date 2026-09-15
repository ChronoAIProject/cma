import type { AgentCreationCategory } from '@chronoai/cma-sdk';
import { useEffect, useMemo, useState } from 'react';
import { useCmaClient } from '../api/context';
import { creationLabels } from '../agent/provenance';
import type { NavigationSandbox } from './navigation';

type CreationFacts = ReadonlyMap<string, AgentCreationCategory | 'unavailable'>;
export type CreationFilter = 'all' | AgentCreationCategory;
const emptyFacts: CreationFacts = new Map();
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
  const client = useCmaClient();
  const identity = JSON.stringify(agentIds(items));
  const request = useMemo(
    () => ({ client, identity, viewerId, enabled }),
    [client, identity, viewerId, enabled],
  );
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
      let next = 0;
      const worker = async () => {
        while (next < ids.length && !lifetime.signal.aborted) {
          const id = ids[next++];
          if (id === undefined) return;
          let category: AgentCreationCategory | 'unavailable' = 'unavailable';
          try {
            const value = await request.client.readAgentProvenance(id, { signal: lifetime.signal });
            if (value.agent_id === id) category = value.creation_origin;
          } catch {
            /* A refused or missing fact never supplies a creation category. */
          }
          if (!lifetime.signal.aborted)
            setState((previous) => ({
              request,
              facts: new Map(previous?.request === request ? previous.facts : emptyFacts).set(
                id,
                category,
              ),
            }));
        }
      };
      await Promise.all(Array.from({ length: Math.min(4, ids.length) }, worker));
      pending = false;
      if (!lifetime.signal.aborted) timer = setTimeout(() => void poll(), 60_000);
    };
    const foreground = () => void poll();
    document.addEventListener('visibilitychange', foreground);
    void poll();
    return () => {
      lifetime.abort();
      clearTimeout(timer);
      document.removeEventListener('visibilitychange', foreground);
    };
  }, [request]);
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

export function filterTreeCreation(
  items: readonly NavigationSandbox[],
  facts: CreationFacts,
  filter: CreationFilter,
): readonly NavigationSandbox[] {
  if (filter === 'all') return items;
  return items.flatMap((item) => {
    const workspaces = item.workspaces
      .filter((workspace) => workspace.status !== 'removed')
      .flatMap((workspace) => {
        const agents = workspace.agents.filter(
          (agent) => agent.status !== 'deleted' && facts.get(agent.id) === filter,
        );
        return agents.length === 0 ? [] : [{ ...workspace, agents }];
      });
    return workspaces.length === 0 ? [] : [{ ...item, workspaces }];
  });
}

export function TreeCreationFilter({
  items,
  facts,
  value,
  onChange,
}: {
  readonly items: readonly NavigationSandbox[];
  readonly facts: CreationFacts;
  readonly value: CreationFilter;
  readonly onChange: (value: CreationFilter) => void;
}) {
  const ids = agentIds(items);
  const pending = ids.filter((id) => !facts.has(id)).length;
  const unavailable = ids.filter((id) => facts.get(id) === 'unavailable').length;
  return (
    <div className="resource-tree__filter">
      <label>
        Agent creation
        <select value={value} onChange={(event) => onChange(event.target.value as CreationFilter)}>
          <option value="all">All origins</option>
          <option value="manual">Manual</option>
          <option value="trigger">Trigger</option>
          <option value="channel_bridge">Channel bridge</option>
          <option value="legacy_unknown">Unknown origin</option>
        </select>
      </label>
      {value === 'all' ? null : (
        <p role="status">
          {pending > 0 ? 'Loading creation origins. ' : ''}
          {unavailable > 0 ? `${String(unavailable)} Agent origins unavailable. ` : ''}
          Filtering Agents in this tree.
        </p>
      )}
    </div>
  );
}
