import type { AgentSummary, SandboxSummary } from '../api/types';
import { sandboxChildStatus } from '../sandbox/lifecycle';
import { taskStatus, taskWords, type TaskStatus } from './taskStatus';

export type EnvironmentTaskGroup =
  'Needs attention' | 'Updating environment' | 'Status unavailable';

export interface TaskPresentation {
  readonly status: TaskStatus;
  readonly group?: EnvironmentTaskGroup;
  readonly label: string;
  readonly parentNotice?: string;
  readonly showRunningTime: boolean;
}

export function taskPresentation(
  agent: AgentSummary,
  sandbox: SandboxSummary,
  freshness: 'fresh' | 'stale',
): TaskPresentation | undefined {
  if (['retiring', 'retired'].includes(sandbox.status) || agent.status === 'deleted')
    return undefined;
  const status = taskStatus(agent, sandbox);
  if (freshness === 'stale')
    return {
      status,
      group: 'Status unavailable',
      label: 'Status unavailable',
      showRunningTime: false,
    };

  const rotating = sandboxChildStatus(sandbox, 'ready') === 'rotating';
  if (rotating) {
    const recorded = taskStatus(agent);
    const ended = recorded === 'failed' || recorded === 'stopped';
    const failed = sandbox.runtimeUpdate?.state === 'failed';
    const group = failed ? 'Needs attention' : 'Updating environment';
    return {
      status: recorded,
      group: ended ? undefined : group,
      label: ended || recorded === 'completed' ? taskWords[recorded] : group,
      parentNotice: failed ? 'Runtime update failed' : 'Updating environment',
      showRunningTime: false,
    };
  }
  return { status, label: taskWords[status], showRunningTime: status === 'working' };
}
