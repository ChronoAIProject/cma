import { selectAgentAction } from './agentActionDispatch';
import { RuntimeAttentionDisclosure } from './RuntimeAttentionDisclosure';
import { runtimeAttention } from './runtimeAttention';
import { CharacterAvatar } from '../character/CharacterAvatar';
import { Button } from '../ui/Button';
import { useEffect, useMemo } from 'react';
import {
  TASK_GROUPS,
  taskGroup,
  taskTime,
  taskColor,
  runningTaskTime,
  taskActivityOrder,
} from './taskGroups';
import { useTaskClock } from './useTaskClock';
import type { TreeProps } from './Tree';
import { TreeRow } from './TreeRow';
import { agentActions } from './treeActions';
import { keyboardNavigation, agentSignal } from './treePresentation';
import { taskPresentation } from './taskPresentation';
import { IconDataOutline16 } from '../ui/icons';
import { sandboxChildStatus } from '../sandbox/lifecycle';
import { taskTitle } from './taskTitle';
import type { AgentDetail, AgentSummary, SandboxDetail } from '../api/types';
import { setupEnvironmentRoute, SETUP_ENVIRONMENT_NOTICE } from '../agent/setupEnvironment';

interface AgentCard {
  readonly sandbox: SandboxDetail;
  readonly agent: AgentSummary;
}

/** Flat navigation keeps each Agent's parent only for status and action eligibility. */
function agentCards(items: TreeProps['items']): readonly AgentCard[] {
  return items
    .flatMap(({ sandbox, workspaces }) =>
      workspaces
        .filter((workspace) => workspace.status !== 'removed')
        .flatMap((workspace) => workspace.agents)
        .map((agent) => ({ sandbox, agent })),
    )
    .sort(
      (a, b) =>
        taskActivityOrder(b.agent) - taskActivityOrder(a.agent) ||
        a.agent.id.localeCompare(b.agent.id),
    );
}

/** Agent view uses the same authorized hierarchy and parent lifecycle barriers. */
export function PersonalAgentTree({
  currentTask,
  selectedRetired,
  onShowAll,
  attention,
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
  | 'selectedRetired'
  | 'onShowAll'
  | 'attention'
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
      agentCards(items).flatMap((card) => {
        const presentation = taskPresentation(
          card.agent,
          card.sandbox,
          refreshError === undefined ? 'fresh' : 'stale',
          currentTask,
        );
        if (
          !presentation ||
          (statusFilter && statusFilter !== 'all' && presentation.status !== statusFilter)
        )
          return [];
        return [{ ...card, presentation }];
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
  const observedAttention = attention ?? runtimeAttention({ status: 'ready', items, refreshError });
  const failedIds = new Set(observedAttention.failedSandboxes.map((sandbox) => sandbox.sandboxId));
  const collapsedCards = cards.filter(
    ({ sandbox, presentation, agent }) =>
      agent.id !== selectedRetired?.id &&
      failedIds.has(sandbox.id) &&
      presentation.group === 'Needs attention' &&
      presentation.status !== 'failed' &&
      presentation.status !== 'waiting_approval',
  );
  const normalCards = cards.filter(
    (card) => !collapsedCards.includes(card) && card.agent.id !== selectedRetired?.id,
  );
  const renderCard = ({ sandbox, agent, presentation }: (typeof cards)[number]) => {
    const title =
      agent.status === 'starting'
        ? agent.ownerTitle?.trim() || agent.title?.trim() || 'New agent'
        : taskTitle(agent);
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
          : taskTime(agent.taskActivityAt, now);
    return (
      <TreeRow
        key={agent.id}
        nodeId={`agent:${agent.id}`}
        level={1}
        label={title}
        accessibleLabel={`${title}, ${label}${parentNotice && parentNotice !== label ? `, ${parentNotice}` : ''}${displayedTime ? `, ${displayedTime}` : ''}`}
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
            {environmentNotice ? null : status === 'starting' || status === 'not_started' ? (
              <span className="task-ready">{status === 'starting' ? 'Preparing' : 'Ready'}</span>
            ) : showRunningTime ? (
              <span className="task-running">
                <span aria-hidden="true" className="task-running__dot" />
                {runningTaskTime(agent.taskStartedAt, now)}
              </span>
            ) : !Number.isFinite(taskActivityOrder(agent)) ? (
              <span>Task time unavailable</span>
            ) : (
              <time dateTime={agent.taskActivityAt}>{taskTime(agent.taskActivityAt, now)}</time>
            )}
          </>
        }
        actions={[
          ...agentActions(agent.status, permissions, {
            sandbox: agent.sandboxId === sandbox.id ? sandbox : undefined,
            viewerId,
            busy: lifecycleBusy || busySandboxIds?.has(sandbox.id),
            stale: refreshError !== undefined,
          }),
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
            : selectAgentAction(
                kind,
                agent.id,
                onAction,
                agent.sandboxId === sandbox.id ? sandbox.id : undefined,
              )
        }
      />
    );
  };
  return (
    <div
      className="agent-navigation"
      role="tree"
      aria-label="Personal Agents"
      onKeyDown={keyboardNavigation}
    >
      <RuntimeAttentionDisclosure
        attention={observedAttention}
        revealAgents={statusFilter === 'needs_attention'}
        selectedSandboxId={route.kind === 'sandbox' ? route.sandboxId : undefined}
        selectedAgentId={route.kind === 'agent' ? route.agentId : undefined}
        onSelect={(sandboxId) => onNavigate({ kind: 'sandbox', sandboxId })}
        renderAgents={(sandboxId) =>
          collapsedCards.filter((card) => card.sandbox.id === sandboxId).map(renderCard)
        }
      />
      {TASK_GROUPS.map((heading) => {
        const rows = normalCards.filter(
          ({ presentation, agent }) =>
            (presentation.group ?? taskGroup(presentation.status, agent.taskActivityAt, now)) ===
            heading,
        );
        if (!rows.length) return null;
        return (
          <div role="group" aria-label={heading} key={heading}>
            <h3 className="task-group__heading">{heading}</h3>
            {rows.map(renderCard)}
          </div>
        );
      })}
      {selectedRetired ? (
        <RetiredAgentRow detail={selectedRetired} onNavigate={onNavigate} />
      ) : null}
      {normalCards.length === 0 && collapsedCards.length === 0 ? (
        <p className="resource-tree__empty">
          {statusFilter && statusFilter !== 'all'
            ? 'No agents match these filters.'
            : selectedRetired
              ? 'No other agents to show.'
              : 'No agents to show.'}
          {statusFilter && statusFilter !== 'all' && onShowAll ? (
            <Button size="sm" onClick={onShowAll}>
              Show all
            </Button>
          ) : null}
        </p>
      ) : null}
    </div>
  );
}

export function RetiredAgentRow({
  detail,
  onNavigate,
}: {
  readonly detail: AgentDetail;
  readonly onNavigate: TreeProps['onNavigate'];
}) {
  return (
    <div role="group" aria-label="Retired">
      <h3 className="task-group__heading">Retired</h3>
      <TreeRow
        nodeId={`agent:${detail.id}`}
        level={1}
        label={taskTitle(detail)}
        accessibleLabel={`${taskTitle(detail)}, Retired`}
        icon={<CharacterAvatar pose="paused" still size={24} />}
        route={{ kind: 'agent', agentId: detail.id }}
        selected
        meta={<span>Shown because you’re viewing it.</span>}
        onNavigate={onNavigate}
      />
    </div>
  );
}
