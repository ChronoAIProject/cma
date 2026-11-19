import { useEffect, useMemo } from 'react';
import { TASK_GROUPS, taskGroup, taskTime, taskColor, runningTaskTime } from './taskGroups';
import { useTaskClock } from './useTaskClock';
import type { TreeProps } from './Tree';
import { TreeRow } from './TreeRow';
import { agentActions } from './treeActions';
import { keyboardNavigation, agentSignal } from './treePresentation';
import { taskWords, observedTaskStatus } from './taskStatus';
import { sandboxChildrenAvailable, sandboxChildStatus } from '../sandbox/lifecycle';
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
function agentCards(
  items: TreeProps['items'],
  statusFilter: string,
  currentTask?: TreeProps['currentTask'],
): readonly AgentCard[] {
  return items
    .flatMap(({ sandbox, workspaces }) =>
      workspaces
        .filter((workspace) => workspace.status !== 'removed')
        .flatMap((workspace) => workspace.agents)
        .filter(
          (agent) =>
            agent.status !== 'deleted' &&
            (statusFilter === 'all' ||
              observedTaskStatus(agent, sandbox, currentTask) === statusFilter),
        )
        .map((agent) => ({ sandbox, agent })),
    )
    .sort((a, b) => updated(b.agent) - updated(a.agent) || a.agent.id.localeCompare(b.agent.id));
}

/** Agent view uses the same authorized hierarchy and parent lifecycle barriers. */
export function PersonalAgentTree({
  currentTask,
  active,
  items,
  statusFilter,
  route,
  permissions,
  lifecycleBusy,
  busySandboxIds,
  refreshError,
  onNavigate,
  onAgentIntent,
  onAction,
}: Pick<
  TreeProps,
  | 'currentTask'
  | 'items'
  | 'active'
  | 'statusFilter'
  | 'route'
  | 'permissions'
  | 'lifecycleBusy'
  | 'busySandboxIds'
  | 'refreshError'
  | 'onNavigate'
  | 'onAgentIntent'
  | 'onAction'
>) {
  const cards = useMemo(
    () => agentCards(items, statusFilter ?? 'all', currentTask),
    [items, statusFilter, currentTask],
  );
  const listPrefetchAgent =
    active && route.kind !== 'agent' && refreshError === undefined ? cards[0]?.agent.id : undefined;
  useEffect(() => {
    if (listPrefetchAgent === undefined || onAgentIntent === undefined) return;
    onAgentIntent(listPrefetchAgent, true);
    return () => onAgentIntent(listPrefetchAgent, false);
  }, [listPrefetchAgent, onAgentIntent]);
  const now = useTaskClock(
    cards
      .filter(({ agent, sandbox }) => observedTaskStatus(agent, sandbox, currentTask) === 'working')
      .flatMap(({ agent }) => (agent.taskStartedAt ? [agent.taskStartedAt] : [])),
  );
  return (
    <div
      className="agent-navigation"
      role="tree"
      aria-label="Personal Agents"
      onKeyDown={keyboardNavigation}
    >
      {TASK_GROUPS.map((heading) => {
        const rows = cards.filter(
          ({ sandbox, agent }) =>
            taskGroup(observedTaskStatus(agent, sandbox, currentTask), agent.updatedAt, now) ===
            heading,
        );
        if (!rows.length) return null;
        return (
          <div role="group" aria-label={heading} key={heading}>
            <h3 className="task-group__heading">{heading}</h3>
            {rows.map(({ sandbox, agent }) => {
              const status = observedTaskStatus(agent, sandbox, currentTask);
              const displayedTime =
                status === 'not_started'
                  ? ''
                  : status === 'working'
                    ? runningTaskTime(agent.taskStartedAt, now)
                    : taskTime(agent.updatedAt, now);
              return (
                <TreeRow
                  key={agent.id}
                  nodeId={`agent:${agent.id}`}
                  level={1}
                  label={taskTitle(agent)}
                  accessibleLabel={`${taskTitle(agent)}, ${taskWords[status]}${displayedTime ? `, ${displayedTime}` : ''}${refreshError ? ', status may be stale' : ''}`}
                  layout="line"
                  icon={agentSignal(
                    sandboxChildStatus(sandbox, agent.status),
                    agent.id,
                    24,
                    sandbox.color ?? taskColor(agent.id),
                  )}
                  route={{ kind: 'agent', agentId: agent.id }}
                  selected={route.kind === 'agent' && route.agentId === agent.id}
                  dimmed={sandbox.status === 'paused'}
                  meta={
                    <>
                      <span className="sr-only">
                        {taskWords[status]}
                        {refreshError ? ', status may be stale' : ''}
                      </span>
                      {status === 'not_started' ? (
                        <span className="task-ready">Ready</span>
                      ) : status === 'working' ? (
                        <span className="task-running">
                          <span aria-hidden="true" className="task-running__dot" />
                          {runningTaskTime(agent.taskStartedAt, now)}
                        </span>
                      ) : (
                        <time dateTime={agent.updatedAt}>{taskTime(agent.updatedAt, now)}</time>
                      )}
                    </>
                  }
                  actions={agentActions(
                    agent.status,
                    permissions,
                    !lifecycleBusy &&
                      !busySandboxIds?.has(sandbox.id) &&
                      !refreshError &&
                      sandboxChildrenAvailable(sandbox),
                  )}
                  onNavigate={onNavigate}
                  onIntentStart={() => onAgentIntent?.(agent.id, true)}
                  onIntentEnd={() => onAgentIntent?.(agent.id, false)}
                  onAction={(kind) =>
                    onAction({
                      kind: kind as 'stop_agent' | 'delete_agent' | 'export_history',
                      agentId: agent.id,
                    })
                  }
                />
              );
            })}
          </div>
        );
      })}
      {cards.length === 0 ? (
        <p className="resource-tree__empty">No Agents match this scope and status.</p>
      ) : null}
    </div>
  );
}
