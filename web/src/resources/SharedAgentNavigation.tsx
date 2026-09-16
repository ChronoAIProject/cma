import { useAccessSummaries } from './useAccessSummaries';
import { ResourceAccessSummary } from './ResourceAccessSummary';
import { useMemo, useState } from 'react';
import type { ResourceViewsClient } from './client';
import type { AppRoute } from '../routing/routes';
import { useAuthorizedRead } from './useAuthorizedRead';
import { ResourceLink } from './ResourceLink';
import { Button } from '../ui/Button';
import { creationLabels } from '../agent/provenance';
import { statusLabel } from '../vocabulary/status';
import { channelLabel } from './channelLabels';

/** Narrow grants are listed as Agents, without fetching unauthorized parents. */
export function SharedAgentNavigation({
  client,
  onNavigate,
}: {
  readonly client: ResourceViewsClient;
  readonly onNavigate: (route: AppRoute) => void;
}) {
  const [cursors, setCursors] = useState<readonly (string | undefined)[]>([undefined]);
  const cursor = cursors.at(-1);
  const read = useMemo(
    () => async (signal: AbortSignal) => {
      const page = await client.agents.provenanceInventory({
        shared: true,
        cursor,
        limit: 50,
        signal,
      });
      if (
        page.next !== undefined &&
        (page.next === '' || (cursor !== undefined && page.next <= cursor))
      )
        throw new Error('Shared Agent pagination did not advance. Refresh this scope.');
      let previous = cursor;
      for (const row of page.items) {
        if (previous !== undefined && row.agent_id <= previous)
          throw new Error('Shared Agent order changed. Refresh this scope.');
        previous = row.agent_id;
      }
      return { value: page };
    },
    [client, cursor],
  );
  const current = useAuthorizedRead(read);
  const accessSummaries = useAccessSummaries(
    client,
    'agent',
    current.state.status === 'ready' ? current.state.value.items.map((row) => row.agent_id) : [],
  );
  return (
    <div className="scoped-navigation__tree">
      <ResourceLink route={{ kind: 'inventory', shared: true }} onNavigate={onNavigate}>
        All shared Agents
      </ResourceLink>
      <Button size="sm" onClick={current.reload}>
        Refresh shared Agents
      </Button>
      {current.state.status === 'loading' ? (
        <p role="status">Loading shared Agents</p>
      ) : current.state.status === 'error' ? (
        <p role="alert">{current.state.message}</p>
      ) : (
        <>
          {current.state.value.items.length === 0 ? (
            <p>No shared Agents on this page.</p>
          ) : (
            <ul aria-label="Shared Agents">
              {current.state.value.items.map((row) => (
                <li key={row.agent_id}>
                  <ResourceLink
                    route={{ kind: 'agent', agentId: row.agent_id }}
                    onNavigate={onNavigate}
                  >
                    {row.title?.trim() || 'Untitled agent'}
                  </ResourceLink>
                  <ResourceAccessSummary value={accessSummaries.get(row.agent_id)} />
                  <p>
                    {statusLabel(row.status)} · {creationLabels[row.creation_origin]} ·{' '}
                    {channelLabel(row.channel_usage)}
                  </p>
                </li>
              ))}
            </ul>
          )}
          <div className="scoped-navigation__pages">
            <Button
              size="sm"
              disabled={cursors.length === 1}
              onClick={() => setCursors((values) => values.slice(0, -1))}
            >
              Previous shared Agents
            </Button>
            <Button
              size="sm"
              disabled={current.state.value.next === undefined}
              onClick={() => {
                if (current.state.status === 'ready') {
                  const next = current.state.value.next;
                  if (next !== undefined) setCursors((values) => [...values, next]);
                }
              }}
            >
              Next shared Agents
            </Button>
          </div>
        </>
      )}
    </div>
  );
}
