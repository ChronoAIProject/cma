import { useAccessSummaries } from './useAccessSummaries';
import { ResourceAccessSummary } from './ResourceAccessSummary';
import { useMemo, useState } from 'react';
import { ApiError, type AgentCreationCategory } from '@chronoai/cma-sdk';
import type { ResourceViewsClient } from './client';
import type { AppRoute } from '../routing/routes';
import { Button } from '../ui/Button';
import { creationLabels } from '../agent/provenance';
import { statusLabel } from '../vocabulary/status';
import { channelLabel, channelQuery, type ChannelFilter } from './channelLabels';
import { useAuthorizedRead } from './useAuthorizedRead';
import { ResourceLink } from './ResourceLink';

export interface SandboxAgentFilters {
  readonly creation: 'all' | AgentCreationCategory;
  readonly channel: ChannelFilter;
}

/** Filter within a listed Sandbox using only the current scope's safe Agent inventory. */
export function SandboxTreeAgents({
  client,
  sandboxId,
  organization,
  shared,
  filters: { creation, channel },
  onNavigate,
  onAccessChanged,
}: {
  readonly client: ResourceViewsClient;
  readonly sandboxId: string;
  readonly organization?: string;
  readonly shared?: boolean;
  readonly filters: SandboxAgentFilters;
  readonly onNavigate: (route: AppRoute) => void;
  readonly onAccessChanged: () => void;
}) {
  const [cursors, setCursors] = useState<readonly (string | undefined)[]>([undefined]);
  const cursor = cursors.at(-1);
  const read = useMemo(
    () => async (signal: AbortSignal) => {
      try {
        const page = await client.agents.provenanceInventory({
          organization,
          shared,
          sandbox: sandboxId,
          creationOrigin: creation === 'all' ? undefined : creation,
          ...channelQuery(channel),
          cursor,
          limit: 50,
          signal,
        });
        if (
          page.next !== undefined &&
          (page.next === '' || (cursor !== undefined && page.next <= cursor))
        )
          throw new Error('Filtered Agent pagination did not advance. Refresh this Sandbox.');
        let previous = cursor;
        for (const row of page.items) {
          if (previous !== undefined && row.agent_id <= previous)
            throw new Error('Filtered Agent order changed. Refresh this Sandbox.');
          previous = row.agent_id;
        }
        return { value: page };
      } catch (error) {
        if (
          !signal.aborted &&
          error instanceof ApiError &&
          [401, 403, 404, 409].includes(error.status)
        )
          onAccessChanged();
        throw error;
      }
    },
    [client, sandboxId, organization, shared, creation, channel, cursor, onAccessChanged],
  );
  const current = useAuthorizedRead(read);
  const accessSummaries = useAccessSummaries(
    client,
    'agent',
    current.state.status === 'ready' ? current.state.value.items.map((row) => row.agent_id) : [],
  );
  return (
    <div>
      <Button size="sm" onClick={current.reload}>
        Refresh filtered Agents
      </Button>
      {current.state.status === 'loading' ? (
        <p role="status">Loading matching Agents</p>
      ) : current.state.status === 'error' ? (
        <p role="alert">{current.state.message}</p>
      ) : (
        <>
          {current.state.value.items.length === 0 ? (
            <p>
              {current.state.value.next === undefined
                ? 'No Agents match these filters in this Sandbox.'
                : 'No matching Agents on this page. Continue to check the next page.'}
            </p>
          ) : (
            <ul aria-label="Matching Agents in this Sandbox">
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
              Previous matching Agents
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
              Next matching Agents
            </Button>
          </div>
        </>
      )}
    </div>
  );
}
