import type { TreeProps } from './Tree';
import { TreeRow } from './TreeRow';
import { agentActions } from './treeActions';
import { activityLabel, keyboardNavigation, stateMeta } from './treePresentation';
import { sandboxChildrenAvailable, sandboxChildStatus } from '../sandbox/lifecycle';
import { agentDisplayTitle } from '../agent/presentation';
import { IconAgentPresetOutline16 } from '../ui/icons';

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
  const agents = items.flatMap(({ sandbox, workspaces }) =>
    workspaces
      .filter((workspace) => workspace.status !== 'removed')
      .flatMap((workspace) =>
        workspace.agents
          .filter(
            (agent) =>
              agent.status !== 'deleted' &&
              (statusFilter === 'all' ||
                sandboxChildStatus(sandbox, agent.status) === statusFilter),
          )
          .map((agent) => ({ agent, sandbox })),
      ),
  );
  return (
    <div role="tree" aria-label="Personal Agents" onKeyDown={keyboardNavigation}>
      {agents.map(({ agent, sandbox }) => (
        <TreeRow
          key={agent.id}
          nodeId={`agent:${agent.id}`}
          level={1}
          label={agentDisplayTitle(agent)}
          icon={<IconAgentPresetOutline16 />}
          route={{ kind: 'agent', agentId: agent.id }}
          selected={route.kind === 'agent' && route.agentId === agent.id}
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
      {agents.length === 0 ? (
        <p className="resource-tree__empty">No Agents match this scope and status.</p>
      ) : null}
    </div>
  );
}
