import type { AgentSummary, SandboxSummary } from '../api/types';
import { sandboxStatusPresentation } from '../sandbox/presentation';
import { observedTaskStatus, taskWords, type TaskStatus, type CurrentTask } from './taskStatus';

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
  currentTask?: CurrentTask,
): TaskPresentation | undefined {
  if (['retiring', 'retired'].includes(sandbox.status) || agent.status === 'deleted')
    return undefined;
  const status = observedTaskStatus(agent, sandbox, currentTask);
  if (freshness === 'stale')
    return {
      status,
      group: 'Status unavailable',
      label: 'Status unavailable',
      showRunningTime: false,
    };

  const parent = sandboxStatusPresentation(sandbox);
  if (parent.status === 'rotating') {
    const recorded = observedTaskStatus(agent, undefined, currentTask);
    const ended = recorded === 'failed' || recorded === 'stopped';
    const failed = sandbox.runtimeUpdate?.state === 'failed';
    const group = failed ? 'Needs attention' : 'Updating environment';
    return {
      status: recorded,
      group: ended ? undefined : group,
      label:
        ended || recorded === 'completed' ? taskWords[recorded] : failed ? group : parent.label,
      parentNotice: failed ? 'Runtime update failed' : parent.label,
      showRunningTime: false,
    };
  }
  return { status, label: taskWords[status], showRunningTime: status === 'working' };
}
