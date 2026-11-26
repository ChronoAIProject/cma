import type { AgentSummary, SandboxSummary } from '../api/types';
import { sandboxStatusPresentation } from '../sandbox/presentation';
import { sandboxHasParkedRuntimeUpdate } from '../sandbox/parkedRuntimeUpdate';
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
      status: 'unknown',
      group: 'Status unavailable',
      label: 'Status unavailable',
      showRunningTime: false,
    };

  const parent = sandboxStatusPresentation(sandbox);
  const failed = sandboxHasParkedRuntimeUpdate(sandbox);
  if (parent.status === 'rotating' || failed) {
    const recorded = observedTaskStatus(agent, undefined, currentTask);
    const ended = recorded === 'failed' || recorded === 'interrupted' || recorded === 'stopped';
    const showRecordedOutcome = ended || recorded === 'completed';
    const group = failed ? 'Needs attention' : 'Updating environment';
    return {
      status: failed && !showRecordedOutcome ? 'needs_attention' : recorded,
      group: ended ? undefined : group,
      label: showRecordedOutcome ? taskWords[recorded] : failed ? group : parent.label,
      parentNotice: failed ? 'Runtime update failed' : parent.label,
      showRunningTime: false,
    };
  }
  return { status, label: taskWords[status], showRunningTime: status === 'working' };
}
