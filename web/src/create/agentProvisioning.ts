import type { AgentLifecycleOperationEvent, AgentLifecycleOperationView } from '../api/client';

export const agentProvisioningSteps = [
  { phase: 'sandbox_ready', label: 'Sandbox' },
  { phase: 'workspace_runtime', label: 'Workspace provisioning' },
  { phase: 'key_scope_ready', label: 'Key permissions' },
  { phase: 'agent_startup', label: 'Agent startup' },
] as const;

export function orderedProvisioningEvents(operation?: AgentLifecycleOperationView) {
  return [...(operation?.events ?? [])].sort((left, right) => left.sequence - right.sequence);
}

export function provisioningStepState(
  phase: string,
  events: readonly AgentLifecycleOperationEvent[],
  operation?: AgentLifecycleOperationView,
) {
  const latest = events.filter((event) => event.phase === phase).at(-1);
  if (
    latest?.status === 'ready' ||
    (phase === 'agent_startup' && operation?.status === 'succeeded' && operation.agent_id)
  )
    return 'Ready';
  if (latest?.status === 'failed') return 'Needs attention';
  if (operation?.status === 'failed')
    return latest?.status === 'running' ? 'Needs attention' : 'Not reported';
  if (operation?.status === 'succeeded') return 'Not reported';
  if (latest?.status !== 'running') return 'Waiting';
  if (operation?.status === 'waiting_for_connection') return 'Waiting for connection';
  if (operation?.status === 'queued') return 'Queued';
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

export function provisioningStepDuration(
  phase: string,
  events: readonly AgentLifecycleOperationEvent[],
  status: string,
  now: number,
) {
  const phaseEvents = events.filter((event) => event.phase === phase);
  const start = phaseEvents.find((event) => event.status === 'running')?.occurred_at;
  const latest = phaseEvents.at(-1);
  const finished = latest?.status === 'ready' || latest?.status === 'failed';
  if (!finished && (status === 'succeeded' || status === 'failed')) return undefined;
  return provisioningDuration(start, finished ? Date.parse(latest.occurred_at) : now);
}
