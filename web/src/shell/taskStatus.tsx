import type { AgentSummary, SandboxSummary } from '../api/types';
import { sandboxChildStatus } from '../sandbox/lifecycle';
import { toneDot } from '../ui/Badge';
import { statusTone } from '../vocabulary/status';

export const taskWords = {
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
        : statusTone(status === 'unknown' ? 'ready' : status);
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
