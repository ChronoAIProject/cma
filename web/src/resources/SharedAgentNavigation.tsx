import { ApiError, type AgentTaskStatus } from '@cma/client';
import { scopeValue } from './scope';
import { useAccessSummaries } from './useAccessSummaries';
import { ResourceAccessSummary } from './ResourceAccessSummary';
import { useMemo, useState } from 'react';
import type { ResourceViewsClient } from './client';
import type { AppRoute } from '../routing/routes';
import { useAuthorizedRead } from './useAuthorizedRead';
import { ResourceLink } from './ResourceLink';
import { Button } from '../ui/Button';
import { creationLabels } from '../agent/provenance';
import { TreeRow } from '../shell/TreeRow';
import '../shell/Tree.css';
import { keyboardNavigation } from '../shell/treePresentation';
import { taskStateMeta, taskStatus, taskWords } from '../shell/taskStatus';
import { taskTitle } from '../shell/taskTitle';
import { IconAgentPresetOutline16 } from '../ui/icons';
import { channelLabel } from './channelLabels';

/** Narrow grants are listed as Agents, without fetching unauthorized parents. */
export function SharedAgentNavigation({
  client,
  onNavigate,
  route,
  organization,
  shared = true,
  statusFilter = 'all',
}: {
  readonly client: ResourceViewsClient;
  readonly organization?: string;
  readonly shared?: boolean;
  readonly statusFilter?: string;
  readonly route?: AppRoute;
  readonly onNavigate: (route: AppRoute) => void;
}) {
  const label = shared ? 'Shared Agents' : 'Organization Agents';
  const [cursors, setCursors] = useState<readonly (string | undefined)[]>([undefined]);
  const cursor = cursors.at(-1);
  const read = useMemo(
    () => async (signal: AbortSignal) => {
      // Leave time to commit the authorized inventory before its 15-second read deadline.
      const deadline = performance.now() + 12_000;
      const page = await client.agents.provenanceInventory({
        shared,
        organization,
        taskStatus: statusFilter === 'all' ? undefined : (statusFilter as AgentTaskStatus),
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
      // The provenance projection can retain the creation placeholder while
      // the authorized point read contains the current title.
      const titles = new Map<string, string>();
      const refused = new Set<string>();
      let next = 0;
      const worker = async () => {
        while (next < page.items.length) {
          signal.throwIfAborted();
          const remaining = Math.floor(deadline - performance.now());
          if (remaining <= 0) return;
          const row = page.items[next++]!;
          try {
            const detail = await client.agents.readAccessible(row.agent_id, {
              signal,
              timeoutMs: Math.min(3000, remaining),
            });
            signal.throwIfAborted();
            titles.set(row.agent_id, taskTitle({ id: detail.id, title: detail.title }));
          } catch (error) {
            signal.throwIfAborted();
            if (error instanceof ApiError && [401, 403, 404, 410].includes(error.status))
              refused.add(row.agent_id);
          }
        }
      };
      await Promise.all(Array.from({ length: Math.min(4, page.items.length) }, worker));
      return {
        value: { ...page, items: page.items.filter((row) => !refused.has(row.agent_id)), titles },
      };
    },
    [client, cursor, organization, shared, statusFilter],
  );
  const current = useAuthorizedRead(read, true);
  const page = current.state.status === 'ready' ? current.state.value : undefined;
  const stale = current.state.status === 'ready' && current.state.stale;
  const accessSummaries = useAccessSummaries(
    client,
    'agent',
    current.state.status === 'ready' && !current.state.stale
      ? current.state.value.items.map((row) => row.agent_id)
      : [],
    scopeValue(organization, shared),
  );
  return (
    <div className="scoped-navigation__tree resource-tree">
      <ResourceLink route={{ kind: 'inventory', shared, organization }} onNavigate={onNavigate}>
        All authorized Agents
      </ResourceLink>
      <Button size="sm" onClick={current.reload}>
        Refresh Agents
      </Button>
      {current.state.status === 'loading' ? (
        <p role="status">Loading Agents</p>
      ) : current.state.status === 'error' ? (
        <p role="alert">{current.state.message}</p>
      ) : (
        <>
          {current.state.refreshError ? <p role="alert">{current.state.refreshError}</p> : null}
          {current.state.value.items.length === 0 ? (
            <p>No Agents match this scope and status on this page.</p>
          ) : (
            <div
              className="agent-navigation shared-agent-navigation"
              role="tree"
              aria-label={label}
              onKeyDown={keyboardNavigation}
            >
              {current.state.value.items.map((row) => (
                <TreeRow
                  key={row.agent_id}
                  nodeId={`agent:${row.agent_id}`}
                  level={1}
                  label={
                    page?.titles.get(row.agent_id) ??
                    taskTitle({
                      id: row.agent_id,
                      title: row.title === 'New Agent' ? undefined : row.title,
                    })
                  }
                  icon={<IconAgentPresetOutline16 />}
                  route={{ kind: 'agent', agentId: row.agent_id }}
                  selected={route?.kind === 'agent' && route.agentId === row.agent_id}
                  disabled={stale}
                  meta={
                    <>
                      <span
                        title={
                          taskWords[taskStatus({ status: row.status, taskStatus: row.task_status })]
                        }
                      >
                        {taskStateMeta(
                          taskStatus({ status: row.status, taskStatus: row.task_status }),
                          stale,
                        )}
                      </span>
                      <span title={creationLabels[row.creation_origin]}>
                        {creationLabels[row.creation_origin]}
                      </span>
                      <span title={channelLabel(row.channel_usage)}>
                        {channelLabel(row.channel_usage)}
                      </span>
                      <ResourceAccessSummary compact value={accessSummaries.get(row.agent_id)} />
                    </>
                  }
                  onNavigate={onNavigate}
                />
              ))}
            </div>
          )}
          <div className="scoped-navigation__pages">
            <Button
              size="sm"
              disabled={current.state.stale || cursors.length === 1}
              onClick={() => setCursors((values) => values.slice(0, -1))}
            >
              Previous Agents
            </Button>
            <Button
              size="sm"
              disabled={current.state.stale || current.state.value.next === undefined}
              onClick={() => {
                if (current.state.status === 'ready') {
                  const next = current.state.value.next;
                  if (next !== undefined) setCursors((values) => [...values, next]);
                }
              }}
            >
              Next Agents
            </Button>
          </div>
        </>
      )}
    </div>
  );
}
