import type { AgentLifecycleCreateReceipt, AgentLifecycleOperationView } from '../api/client';
import type { AgentVisualAction } from '../brand/agentVisual';
import { orderedProvisioningEvents } from '../create/agentProvisioning';

export interface PreparationFacts {
  readonly receipt?: AgentLifecycleCreateReceipt;
  readonly requestAccepted?: boolean;
  readonly operation?: AgentLifecycleOperationView;
  readonly agentId?: string;
  readonly connectionRequired?: boolean;
  readonly readError?: string;
  readonly setupFailure?: string;
  readonly canonicalTitle?: string;
  readonly initialTask?: 'not-started' | 'started' | 'unknown';
  readonly environmentReady?: boolean;
}
export type PreparationStage =
  'checking' | 'preparing' | 'interrupted' | 'waiting' | 'failed' | 'stalled' | 'ready' | 'started';

const setupActions: Partial<Record<PreparationStage, AgentVisualAction>> = {
  preparing: 'preparing',
  waiting: 'question',
  failed: 'failed',
};

export function derivePreparationView(facts: PreparationFacts) {
  const operation =
    facts.receipt && facts.operation?.id === facts.receipt.id ? facts.operation : undefined;
  const accepted = Boolean(facts.receipt) || facts.requestAccepted === true;
  const started = facts.initialTask === 'started';
  const ready = facts.environmentReady === true || operation?.status === 'succeeded';
  const waiting =
    facts.connectionRequired === true || operation?.status === 'waiting_for_connection';
  const agentReady =
    operation?.status === 'succeeded' &&
    Boolean(operation.agent_id?.trim()) &&
    (facts.agentId === undefined || operation.agent_id === facts.agentId);
  const failed = facts.setupFailure !== undefined || operation?.status === 'failed';
  const stage: PreparationStage = started
    ? 'started'
    : !accepted && !waiting
      ? 'checking'
      : failed
        ? 'failed'
        : facts.readError
          ? 'interrupted'
          : waiting
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
  const action = stage === 'ready' && agentReady ? 'ready' : (setupActions[stage] ?? 'rest');
  return {
    stage,
    receipt: facts.receipt,
    operation,
    title: facts.canonicalTitle?.trim() || 'New agent',
    label: labels[stage],
    visual: { action, label: labels[stage] },
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
        ? (facts.setupFailure ??
          operation?.error ??
          'The Agent could not finish setup. Review the setup before trying again.')
        : facts.readError,
  };
}
export type PreparationPresentation = ReturnType<typeof derivePreparationView>;
