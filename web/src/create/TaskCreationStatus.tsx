import type { ReactNode } from 'react';
import type { AgentLifecycleOperationView } from '../api/client';
import type { Intent } from './lifecycleIntentStorage';
import { DocsLink } from '../services/DocsLink';
import { AgentProvisioningProgress } from './AgentProvisioningProgress';
import { DiscardWaitingCreation } from './DiscardWaitingCreation';
import { PreparationView } from '../preparation/PreparationView';
import { derivePreparationView } from '../preparation/preparationPresentation';

export function TaskCreationStatus({
  intent,
  operation,
  failure,
  pending,
  resolvedCommit,
  recovery,
  draft,
  titleId,
  hasFollowUp,
  cancelWaiting,
  onDiscardWaiting,
}: {
  readonly intent?: Intent;
  readonly operation?: AgentLifecycleOperationView;
  readonly failure?: string;
  readonly pending: boolean;
  readonly hasFollowUp?: boolean;
  readonly cancelWaiting?: boolean;
  readonly onDiscardWaiting?: () => void;
  readonly resolvedCommit?: string;
  readonly recovery?: ReactNode;
  readonly draft?: ReactNode;
  readonly titleId?: string;
}) {
  if (!intent) return null;
  return (
    <PreparationView
      titleId={titleId}
      model={derivePreparationView({
        receipt: intent.receipt,
        operation,
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
                {cancelWaiting
                  ? 'Keep or copy your follow-up draft, then clear it to cancel setup.'
                  : 'Keep or copy your follow-up draft, then clear it to discard this task.'}
              </p>
            ) : (
              <DiscardWaitingCreation
                key={intent.key}
                label="task"
                remote={cancelWaiting}
                onDiscard={onDiscardWaiting}
              />
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
          {intent.receipt ? (
            <AgentProvisioningProgress
              receipt={intent.receipt}
              operation={operation}
              announce={false}
              presentation="task"
            />
          ) : null}
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
