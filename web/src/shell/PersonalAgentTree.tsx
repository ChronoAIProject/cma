import type { TreeProps } from './Tree';
import { TreeRow } from './TreeRow';
import { agentActions } from './treeActions';
import { activityLabel, keyboardNavigation, stateMeta } from './treePresentation';
import { sandboxChildrenAvailable, sandboxChildStatus } from '../sandbox/lifecycle';
import { agentDisplayTitle } from '../agent/presentation';
import type { AgentSummary, SandboxDetail } from '../api/types';
import { IconAgentPresetOutline16, IconDataOutline16 } from '../ui/icons';

export interface AgentGroup {
  readonly sandbox: SandboxDetail;
  readonly agents: readonly AgentSummary[];
}

function updated(agent: Pick<AgentSummary, 'updatedAt'>): number {
  const at = Date.parse(agent.updatedAt);
  return Number.isFinite(at) ? at : 0;
}

/**
 * Agents grouped by where they live, most recently active first.
 *
 * A flat list gave two Agents with the same title no way to be told apart.
 * Grouping by Sandbox answers "which one is this" without
 * opening it, and ordering by recency puts the conversation somebody is most
 * likely returning to at the top.
 */
export function groupAgents(
  items: TreeProps['items'],
  statusFilter: string,
): readonly AgentGroup[] {
  const groups = items
    .map(({ sandbox, workspaces }) => ({
      sandbox,
      agents: workspaces
        .filter((workspace) => workspace.status !== 'removed')
        .flatMap((workspace) => workspace.agents)
        .filter(
          (agent) =>
            agent.status !== 'deleted' &&
            (statusFilter === 'all' || sandboxChildStatus(sandbox, agent.status) === statusFilter),
        )
        .sort((a, b) => updated(b) - updated(a)),
    }))
    .filter(({ agents }) => agents.length > 0);
  const latest = (group: AgentGroup) =>
    group.agents[0] === undefined ? 0 : updated(group.agents[0]);
  return [...groups].sort((a, b) => latest(b) - latest(a));
}

/** Agent view uses the same authorized hierarchy and parent lifecycle barriers. */
export function PersonalAgentTree({
  items,
  statusFilter,
  route,
  permissions,
  lifecycleBusy,
  busySandboxIds,
  refreshError,
  onNavigate,
  onAction,
}: Pick<
  TreeProps,
  | 'items'
  | 'statusFilter'
  | 'route'
  | 'permissions'
  | 'lifecycleBusy'
  | 'busySandboxIds'
  | 'refreshError'
  | 'onNavigate'
  | 'onAction'
>) {
  const groups = groupAgents(items, statusFilter ?? 'all');
  return (
    <div role="tree" aria-label="Personal Agents" onKeyDown={keyboardNavigation}>
      {groups.map(({ sandbox, agents }) => (
        <div key={sandbox.id} className="agent-group" role="group" aria-label={sandbox.name}>
          <div className="agent-group__heading" aria-hidden="true">
            <IconDataOutline16 />
            <span className="agent-group__name">{sandbox.name}</span>
          </div>
          {agents.map((agent) => (
            <TreeRow
              key={agent.id}
              nodeId={`agent:${agent.id}`}
              level={1}
              label={agentDisplayTitle(agent)}
              icon={<IconAgentPresetOutline16 />}
              route={{ kind: 'agent', agentId: agent.id }}
              selected={route.kind === 'agent' && route.agentId === agent.id}
              dimmed={sandbox.status === 'paused'}
              meta={stateMeta(
                sandboxChildStatus(sandbox, agent.status),
                <time dateTime={agent.updatedAt}>{activityLabel(agent.updatedAt)}</time>,
              )}
              actions={agentActions(
                agent.status,
                permissions,
                !lifecycleBusy &&
                  !busySandboxIds?.has(sandbox.id) &&
                  !refreshError &&
                  sandboxChildrenAvailable(sandbox),
              )}
              onNavigate={onNavigate}
              onAction={(kind) =>
                onAction({
                  kind: kind as 'stop_agent' | 'delete_agent' | 'export_history',
                  agentId: agent.id,
                })
              }
            />
          ))}
        </div>
      ))}
      {groups.length === 0 ? (
        <p className="resource-tree__empty">No Agents match this scope and status.</p>
      ) : null}
    </div>
  );
}
