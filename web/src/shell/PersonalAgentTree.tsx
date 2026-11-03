import type { TreeProps } from './Tree';
import { TreeRow } from './TreeRow';
import { agentActions } from './treeActions';
import { keyboardNavigation } from './treePresentation';
import { taskStateMeta, taskStatus } from './taskStatus';
import { sandboxChildrenAvailable } from '../sandbox/lifecycle';
import { taskTitle } from './taskTitle';
import type { AgentSummary, SandboxDetail } from '../api/types';

interface AgentCard {
  readonly sandbox: SandboxDetail;
  readonly agent: AgentSummary;
}

function updated(agent: Pick<AgentSummary, 'updatedAt'>): number {
  const at = Date.parse(agent.updatedAt);
  return Number.isFinite(at) ? at : 0;
}

/** Flat navigation keeps each Agent's parent only for status and action eligibility. */
function agentCards(items: TreeProps['items'], statusFilter: string): readonly AgentCard[] {
  return items
    .flatMap(({ sandbox, workspaces }) =>
      workspaces
        .filter((workspace) => workspace.status !== 'removed')
        .flatMap((workspace) => workspace.agents)
        .filter(
          (agent) =>
            agent.status !== 'deleted' &&
            (statusFilter === 'all' || taskStatus(agent, sandbox) === statusFilter),
        )
        .map((agent) => ({ sandbox, agent })),
    )
    .sort((a, b) => updated(b.agent) - updated(a.agent) || a.agent.id.localeCompare(b.agent.id));
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
  const cards = agentCards(items, statusFilter ?? 'all');
  return (
    <div
      className="agent-navigation"
      role="tree"
      aria-label="Personal Agents"
      onKeyDown={keyboardNavigation}
    >
      {cards.map(({ sandbox, agent }) => {
        const status = taskStatus(agent, sandbox);
        return (
          <TreeRow
            key={agent.id}
            nodeId={`agent:${agent.id}`}
            level={1}
            label={taskTitle(agent)}
            icon={null}
            route={{ kind: 'agent', agentId: agent.id }}
            selected={route.kind === 'agent' && route.agentId === agent.id}
            dimmed={sandbox.status === 'paused'}
            meta={taskStateMeta(status, refreshError !== undefined)}
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
        );
      })}
      {cards.length === 0 ? (
        <p className="resource-tree__empty">No Agents match this scope and status.</p>
      ) : null}
    </div>
  );
}
