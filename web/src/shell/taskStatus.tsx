import type { AgentDetail, AgentSummary, SandboxSummary } from '../api/types';
import type { ActivityStatus } from '../agent/activityStatus';
import { sameAgent } from '../agent/status';
import { sandboxChildStatus } from '../sandbox/lifecycle';
import { toneDot } from '../ui/Badge';
import { statusTone } from '../vocabulary/status';

export const taskWords = {
  loading: 'Loading activity',
  not_started: 'Ready to start',
  completed: 'Completed',
  working: 'Working',
  waiting_approval: 'Waiting for approval',
  starting: 'Preparing',
  paused: 'Paused',
  stopped: 'Stopped',
  failed: 'Failed',
  unknown: 'Status unavailable',
} as const;

export type TaskStatus = keyof typeof taskWords;

export interface CurrentTask {
  readonly agent: AgentDetail;
  readonly status: ActivityStatus;
}

export function observedTaskStatus(
  agent: AgentSummary,
  sandbox: SandboxSummary,
  current?: CurrentTask,
): TaskStatus {
  const status = sandboxChildStatus(sandbox, agent.status);
  return current !== undefined &&
    sameAgent(current.agent, agent) &&
    (status === 'ready' || status === 'working' || status === 'waiting_approval')
    ? current.status
    : taskStatus(agent, sandbox);
}

export function taskStatus(
  agent: Pick<AgentSummary, 'status' | 'taskStatus'>,
  sandbox?: SandboxSummary,
): TaskStatus {
  if (agent.status === 'ready' && (agent.taskStatus === 'failed' || agent.taskStatus === 'stopped'))
    return agent.taskStatus;
  const status = sandbox === undefined ? agent.status : sandboxChildStatus(sandbox, agent.status);
  if (status === 'ready') return agent.taskStatus ?? 'unknown';
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
      : status === 'completed'
        ? 'completed'
        : statusTone(status === 'unknown' || status === 'loading' ? 'ready' : status);
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
