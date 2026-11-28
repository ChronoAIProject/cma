import type { ReactNode } from 'react';
import type { AgentLifecycleOperationView } from '../api/client';
import type { Intent } from './lifecycleIntentStorage';
import { DocsLink } from '../services/DocsLink';
import { DiscardWaitingCreation } from './DiscardWaitingCreation';
import { PreparationView, type PreparationMotionContext } from '../preparation/PreparationView';
import { derivePreparationView } from '../preparation/preparationPresentation';

export function TaskQuotaNotice({
  quotaUsage,
}: {
  readonly quotaUsage?: NonNullable<AgentLifecycleOperationView['failure']>['quota_usage'];
}) {
  return (
    <p role="alert">
      The Sandbox environment limit is now reached. Your task draft is saved. Manage environments
      and check capacity before starting again.
      {quotaUsage
        ? ` Active: ${quotaUsage.active}. Deletions using slots: ${quotaUsage.retiring_charged}. Deletions that released slots: ${quotaUsage.retiring_released}.`
        : null}
    </p>
  );
}

export function TaskCreationStatus({
  intent,
  operation: observation,
  motion,
  connectionRequired,
  failure,
  pending,
  resolvedCommit,
  recovery,
  draft,
  titleId,
  hasFollowUp,
  onDiscardWaiting,
}: {
  readonly intent?: Intent;
  readonly operation?: AgentLifecycleOperationView;
  readonly motion?: PreparationMotionContext;
  readonly connectionRequired?: boolean;
  readonly failure?: string;
  readonly pending: boolean;
  readonly hasFollowUp?: boolean;
  readonly onDiscardWaiting?: () => void;
  readonly resolvedCommit?: string;
  readonly recovery?: ReactNode;
  readonly draft?: ReactNode;
  readonly titleId?: string;
}) {
  if (!intent) return null;
  const operation =
    observation?.id === intent.receipt?.id &&
    (intent.agentId === undefined || observation?.agent_id === intent.agentId)
      ? observation
      : undefined;
  return (
    <PreparationView
      titleId={titleId}
      motion={motion && { ...motion, requestKey: intent.key, agentId: intent.agentId }}
      model={derivePreparationView({
        receipt: intent.receipt,
        operation,
        agentId: intent.agentId,
        connectionRequired: !intent.receipt && connectionRequired,
        readError: failure,
        initialTask: !failure && operation?.status === 'queued' ? 'not-started' : 'unknown',
      })}
      message={intent.body.initial_message}
      actions={
        <>
          {recovery}
          {onDiscardWaiting ? (
            hasFollowUp ? (
              <p role="status">
                Keep or copy your follow-up draft, then clear it to discard this task.
              </p>
            ) : (
              <DiscardWaitingCreation key={intent.key} label="task" onDiscard={onDiscardWaiting} />
            )
          ) : null}
          {!intent.receipt && !pending ? (
            <DocsLink view="recover-and-continue">Help recovering this request</DocsLink>
          ) : null}
        </>
      }
      draft={draft}
      details={
        <>
          <dl aria-label="Requested environment">
            <div>
              <dt>Run target</dt>
              <dd>
                {intent.body.machine_included || intent.body.desktop
                  ? 'Shared desktop'
                  : 'Cloud computer'}
              </dd>
            </div>
          </dl>
          {intent.body.repository ? (
            <section aria-label="Requested repository">
              <p className="task-submitted__literal">{intent.body.repository.url}</p>
              <dl>
                <div>
                  <dt>Requested ref</dt>
                  <dd>{intent.body.repository.ref ?? 'Default branch'}</dd>
                </div>
                <div>
                  <dt>Checkout commit</dt>
                  <dd>{resolvedCommit ?? 'Not prepared yet'}</dd>
                </div>
              </dl>
            </section>
          ) : null}
        </>
      }
    />
  );
}
