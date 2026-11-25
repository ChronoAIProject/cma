import type { AgentLifecycleCreateReceipt, AgentLifecycleOperationView } from '../api/client';
import { orderedProvisioningEvents } from '../create/agentProvisioning';

export interface PreparationFacts {
  readonly receipt?: AgentLifecycleCreateReceipt;
  readonly requestAccepted?: boolean;
  readonly operation?: AgentLifecycleOperationView;
  readonly readError?: string;
  readonly setupFailure?: string;
  readonly canonicalTitle?: string;
  readonly initialTask?: 'not-started' | 'started' | 'unknown';
  readonly environmentReady?: boolean;
}
export type PreparationStage =
  'checking' | 'preparing' | 'interrupted' | 'waiting' | 'failed' | 'stalled' | 'ready' | 'started';

export function derivePreparationView(facts: PreparationFacts) {
  const operation =
    facts.receipt && facts.operation?.id === facts.receipt.id ? facts.operation : undefined;
  const accepted = Boolean(facts.receipt) || facts.requestAccepted === true;
  const started = facts.initialTask === 'started';
  const ready = facts.environmentReady === true || operation?.status === 'succeeded';
  const failed = facts.setupFailure !== undefined || operation?.status === 'failed';
  const stage: PreparationStage = started
    ? 'started'
    : !accepted
      ? 'checking'
      : failed
        ? 'failed'
        : facts.readError
          ? 'interrupted'
          : operation?.status === 'waiting_for_connection'
            ? 'waiting'
            : operation?.phase === 'recoverable_pending'
              ? 'stalled'
              : ready
                ? 'ready'
                : 'preparing';
  const labels: Record<PreparationStage, string> = {
    checking: 'Checking the original request',
    preparing: 'Preparing',
    interrupted: 'Setup updates interrupted',
    waiting: 'Waiting for connection',
    failed: 'Setup needs attention',
    stalled: 'Setup is taking longer than expected',
    ready: 'Cloud computer ready',
    started: 'Started',
  };
  const last = orderedProvisioningEvents(operation)
    .filter((event) => Number.isFinite(Date.parse(event.occurred_at)))
    .at(-1);
  const acceptedAt =
    operation?.accepted_at && Number.isFinite(Date.parse(operation.accepted_at))
      ? operation.accepted_at
      : undefined;
  return {
    stage,
    title: facts.canonicalTitle?.trim() || 'New agent',
    label: labels[stage],
    headline: stage === 'preparing' ? 'Getting its cloud computer ready' : labels[stage],
    accepted,
    acceptedAt,
    progressAt: last?.occurred_at,
    ready,
    started,
    animate: stage === 'preparing',
    notStarted: accepted && stage !== 'failed' && facts.initialTask === 'not-started',
    error:
      stage === 'failed'
        ? (facts.setupFailure ?? operation?.error ?? 'Review the environment before trying again.')
        : facts.readError,
  };
}
export type PreparationPresentation = ReturnType<typeof derivePreparationView>;
