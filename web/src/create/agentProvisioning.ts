import type { AgentLifecycleOperationEvent, AgentLifecycleOperationView } from '../api/client';
import type { StateDotState } from '../ui/StateDot';

export const agentProvisioningSteps = [
  { phase: 'sandbox_ready', label: 'Sandbox' },
  { phase: 'workspace_runtime', label: 'Workspace' },
  { phase: 'key_scope_ready', label: 'Service access' },
  { phase: 'agent_startup', label: 'Agent startup' },
] as const;

export type ProvisioningStepState =
  | 'Waiting'
  | 'Queued'
  | 'In progress'
  | 'Retrying'
  | 'Ready'
  | 'Waiting for connection'
  | 'Needs attention'
  | 'Not reported';

/** The dot reinforces the state word; a step nobody has reported stays neutral. */
export function provisioningStepDot(state: ProvisioningStepState): StateDotState {
  switch (state) {
    case 'Ready':
      return 'done';
    case 'In progress':
    case 'Retrying':
      return 'ongoing';
    case 'Waiting for connection':
      return 'warning';
    case 'Needs attention':
      return 'error';
    default:
      return 'neutral';
  }
}

export function orderedProvisioningEvents(operation?: AgentLifecycleOperationView) {
  return [...(operation?.events ?? [])].sort((left, right) => left.sequence - right.sequence);
}

export function provisioningStepState(
  phase: string,
  events: readonly AgentLifecycleOperationEvent[],
  status: string,
  operation?: AgentLifecycleOperationView,
): ProvisioningStepState {
  const latest = events.filter((event) => event.phase === phase).at(-1);
  if (
    latest?.status === 'ready' ||
    (phase === 'agent_startup' && status === 'succeeded' && operation?.agent_id)
  )
    return 'Ready';
  if (latest?.status === 'failed') return 'Needs attention';
  if (status === 'failed') return latest?.status === 'running' ? 'Needs attention' : 'Not reported';
  if (status === 'succeeded') return 'Not reported';
  if (latest?.status !== 'running') return 'Waiting';
  if (status === 'waiting_for_connection') return 'Waiting for connection';
  if (status === 'queued') return operation?.phase === phase ? 'Retrying' : 'Queued';
  return 'In progress';
}

export function provisioningDuration(start: string | undefined, end: number): string | undefined {
  const beginning = start === undefined ? NaN : Date.parse(start);
  if (!Number.isFinite(beginning) || !Number.isFinite(end)) return undefined;
  const seconds = Math.max(0, Math.floor((end - beginning) / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  return minutes < 60
    ? `${minutes}m ${seconds % 60}s`
    : `${Math.floor(minutes / 60)}h ${minutes % 60}m ${seconds % 60}s`;
}

export function provisioningStepTiming(
  phase: string,
  events: readonly AgentLifecycleOperationEvent[],
  status: string,
  now: number,
) {
  const phaseEvents = events.filter((event) => event.phase === phase);
  const start = phaseEvents.find((event) => event.status === 'running')?.occurred_at;
  const latest = phaseEvents.at(-1);
  const finished = latest?.status === 'ready' || latest?.status === 'failed';
  if (!finished && (status === 'succeeded' || status === 'failed')) return {};
  return {
    start,
    duration: provisioningDuration(start, finished ? Date.parse(latest.occurred_at) : now),
  };
}
