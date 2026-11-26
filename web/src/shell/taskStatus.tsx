import type { AgentDetail, AgentSummary, SandboxSummary } from '../api/types';
import type { ActivityStatus } from '../agent/activityStatus';
import { sameAgent } from '../agent/status';
import { sandboxChildStatus } from '../sandbox/lifecycle';
import { toneDot } from '../ui/Badge';
import { statusTone } from '../vocabulary/status';
import { acceptanceForTurn, taskAcceptanceStatus } from '../agent/taskAcceptance';

export const taskWords = {
  loading: 'Loading activity',
  not_started: 'Ready to start',
  completed: 'Turn ended',
  task_accepted: 'Task complete',
  needs_attention: 'Needs attention',
  working: 'Working',
  waiting_approval: 'Waiting for approval',
  starting: 'Preparing',
  paused: 'Paused',
  interrupted: 'Response interrupted',
  stopped: 'Stopped',
  failed: 'Failed',
  unknown: 'Status unavailable',
} as const;

export type TaskStatus = keyof typeof taskWords;

export interface CurrentTask {
  readonly agent: AgentDetail;
  readonly status?: ActivityStatus;
  readonly turnId?: string;
}

export function observedTaskStatus(
  agent: AgentSummary,
  sandbox: SandboxSummary | undefined,
  current?: CurrentTask,
): TaskStatus {
  const status = sandbox === undefined ? agent.status : sandboxChildStatus(sandbox, agent.status);
  if (
    current !== undefined &&
    sameAgent(current.agent, agent) &&
    (status === 'ready' || status === 'working' || status === 'waiting_approval')
  ) {
    const acceptance = acceptanceForTurn(
      current.agent.taskAcceptance ?? agent.taskAcceptance,
      current.turnId,
    );
    if (current.status !== undefined) {
      const verdict =
        status === 'ready' && current.status === 'completed'
          ? taskAcceptanceStatus(acceptance)
          : undefined;
      return verdict ?? current.status;
    }
    if (status === 'ready' && current.turnId !== undefined)
      return taskStatus({ ...agent, taskAcceptance: acceptance }, sandbox);
  }
  return taskStatus(agent, sandbox);
}

export function taskStatus(
  agent: Pick<AgentSummary, 'status' | 'taskStatus' | 'taskAcceptance'>,
  sandbox?: SandboxSummary,
): TaskStatus {
  if (agent.status === 'ready') {
    if (agent.taskStatus === 'failed') return 'failed';
    // The inventory uses the same wire value for a turn interruption and an
    // Agent lifecycle stop. A Ready Agent with this outcome is still usable.
    if (agent.taskStatus === 'stopped') return 'interrupted';
  }
  const status = sandbox === undefined ? agent.status : sandboxChildStatus(sandbox, agent.status);
  if (status === 'ready') {
    const verdict =
      agent.taskStatus === 'completed' ? taskAcceptanceStatus(agent.taskAcceptance) : undefined;
    return verdict ?? agent.taskStatus ?? 'unknown';
  }
  if (status === 'pausing') return 'paused';
  if (
    status === 'working' ||
    status === 'waiting_approval' ||
    status === 'paused' ||
    status === 'stopped' ||
    status === 'failed'
  )
    return status;
  if (
    status === 'deleted' ||
    status === 'retiring' ||
    status === 'retired' ||
    status === 'removed' ||
    status === 'removing'
  )
    return 'stopped';
  return 'starting';
}

export function taskStateMeta(status: TaskStatus, stale = false) {
  const tone =
    status === 'not_started'
      ? 'ready'
      : status === 'completed' || status === 'task_accepted'
        ? 'completed'
        : status === 'needs_attention'
          ? 'warning'
          : statusTone(
              status === 'unknown' || status === 'loading'
                ? 'ready'
                : status === 'interrupted'
                  ? 'stopped'
                  : status,
            );
  return (
    <>
      <span className="sr-only">, </span>
      <span className="tree-row__state" data-tone={stale ? 'muted' : tone}>
        {stale ? null : tone === 'ready' || tone === 'completed' ? (
          <span className="task-state-dot" data-tone={tone} aria-hidden="true" />
        ) : (
          toneDot(tone)
        )}
        {taskWords[status]}
      </span>
    </>
  );
}
