import { useEffect, useState } from 'react';
import markMarkup from '../assets/brand/cma-mark.svg?raw';
import { TASK_GROUPS, taskGroup, taskTime, taskHue } from './taskGroups';
const markHtml = { __html: markMarkup };
import type { TreeProps } from './Tree';
import { TreeRow } from './TreeRow';
import { agentActions } from './treeActions';
import { keyboardNavigation } from './treePresentation';
import { taskWords, taskStatus } from './taskStatus';
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
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    const timer = setInterval(() => setNow(new Date()), 1000);
    return () => clearInterval(timer);
  }, []);
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
            taskGroup(taskStatus(agent, sandbox), agent.updatedAt, now) === heading,
        );
        if (!rows.length) return null;
        return (
          <div role="group" aria-label={heading} key={heading}>
            <h3 className="task-group__heading">{heading}</h3>
            {rows.map(({ sandbox, agent }) => {
              const status = taskStatus(agent, sandbox);
              return (
                <TreeRow
                  key={agent.id}
                  nodeId={`agent:${agent.id}`}
                  level={1}
                  label={taskTitle(agent)}
                  accessibleLabel={`${taskTitle(agent)}, ${taskWords[status]}, ${agent.updatedAt}${refreshError ? ', status may be stale' : ''}`}
                  layout="line"
                  icon={
                    <span
                      className="task-mask"
                      style={{ filter: `hue-rotate(${taskHue(agent.id)}deg)` }}
                      dangerouslySetInnerHTML={markHtml}
                    />
                  }
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
                          {agent.taskStartedAt && Number.isFinite(Date.parse(agent.taskStartedAt))
                            ? `${Math.floor(Math.max(0, now.getTime() - Date.parse(agent.taskStartedAt)) / 60000)}m`
                            : null}
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
