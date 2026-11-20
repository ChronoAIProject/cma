import { useEffect, useMemo } from 'react';
import { TASK_GROUPS, taskGroup, taskTime, taskColor, runningTaskTime } from './taskGroups';
import { useTaskClock } from './useTaskClock';
import type { TreeProps } from './Tree';
import { TreeRow } from './TreeRow';
import { agentActions } from './treeActions';
import { keyboardNavigation, agentSignal } from './treePresentation';
import { observedTaskStatus } from './taskStatus';
import { taskPresentation } from './taskPresentation';
import { IconDataOutline16 } from '../ui/icons';
import { sandboxChildrenAvailable, sandboxChildStatus } from '../sandbox/lifecycle';
import { taskTitle } from './taskTitle';
import type { AgentSummary, SandboxDetail } from '../api/types';
import { setupEnvironmentRoute, SETUP_ENVIRONMENT_NOTICE } from '../agent/setupEnvironment';

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
  viewerId,
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
  | 'viewerId'
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
    () =>
      agentCards(items, statusFilter ?? 'all', currentTask).flatMap((card) => {
        const presentation = taskPresentation(
          card.agent,
          card.sandbox,
          refreshError === undefined ? 'fresh' : 'stale',
          currentTask,
        );
        return presentation ? [{ ...card, presentation }] : [];
      }),
    [items, statusFilter, refreshError, currentTask],
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
      .filter(({ presentation }) => presentation.showRunningTime)
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
          ({ presentation, agent }) =>
            (presentation.group ?? taskGroup(presentation.status, agent.updatedAt, now)) ===
            heading,
        );
        if (!rows.length) return null;
        return (
          <div role="group" aria-label={heading} key={heading}>
            <h3 className="task-group__heading">{heading}</h3>
            {rows.map(({ sandbox, agent, presentation }) => {
              const setupRoute = setupEnvironmentRoute(
                agent.status,
                agent.sandboxId,
                viewerId,
                permissions.deleteSandbox,
                { sandbox, stale: refreshError !== undefined },
              );
              const { status, label, parentNotice, showRunningTime } = presentation;
              const environmentNotice =
                parentNotice !== undefined || presentation.group === 'Status unavailable';
              const displayedTime =
                environmentNotice || status === 'not_started'
                  ? ''
                  : showRunningTime
                    ? runningTaskTime(agent.taskStartedAt, now)
                    : taskTime(agent.updatedAt, now);
              return (
                <TreeRow
                  key={agent.id}
                  nodeId={`agent:${agent.id}`}
                  level={1}
                  label={taskTitle(agent)}
                  accessibleLabel={`${taskTitle(agent)}, ${label}${parentNotice && parentNotice !== label ? `, ${parentNotice}` : ''}${displayedTime ? `, ${displayedTime}` : ''}`}
                  layout={environmentNotice ? 'stacked' : 'line'}
                  icon={
                    environmentNotice ? (
                      <IconDataOutline16 />
                    ) : (
                      agentSignal(
                        sandboxChildStatus(sandbox, agent.status),
                        agent.id,
                        24,
                        sandbox.color ?? taskColor(agent.id),
                      )
                    )
                  }
                  route={{ kind: 'agent', agentId: agent.id }}
                  selected={route.kind === 'agent' && route.agentId === agent.id}
                  dimmed={sandbox.status === 'paused'}
                  meta={
                    <>
                      {environmentNotice ? (
                        <span className="task-environment-notice">
                          {label}
                          {parentNotice && parentNotice !== label ? ` · ${parentNotice}` : ''}
                        </span>
                      ) : (
                        <span className="sr-only">{label}</span>
                      )}
                      {environmentNotice ? null : status === 'not_started' ? (
                        <span className="task-ready">Ready</span>
                      ) : showRunningTime ? (
                        <span className="task-running">
                          <span aria-hidden="true" className="task-running__dot" />
                          {runningTaskTime(agent.taskStartedAt, now)}
                        </span>
                      ) : (
                        <time dateTime={agent.updatedAt}>{taskTime(agent.updatedAt, now)}</time>
                      )}
                    </>
                  }
                  actions={[
                    ...agentActions(
                      agent.status,
                      permissions,
                      !lifecycleBusy &&
                        !busySandboxIds?.has(sandbox.id) &&
                        !refreshError &&
                        sandboxChildrenAvailable(sandbox),
                    ),
                    ...(setupRoute || environmentNotice
                      ? [
                          {
                            id: 'view_environment',
                            label: setupRoute ? 'Review environment' : 'View environment',
                            reason: setupRoute ? SETUP_ENVIRONMENT_NOTICE : undefined,
                            icon: <IconDataOutline16 />,
                          },
                        ]
                      : []),
                  ]}
                  onNavigate={onNavigate}
                  onIntentStart={() => onAgentIntent?.(agent.id, true)}
                  onIntentEnd={() => onAgentIntent?.(agent.id, false)}
                  onAction={(kind) =>
                    kind === 'view_environment'
                      ? onNavigate({ kind: 'sandbox', sandboxId: sandbox.id })
                      : onAction({
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
