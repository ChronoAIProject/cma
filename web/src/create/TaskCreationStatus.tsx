import type { ReactNode } from 'react';
import type { AgentLifecycleOperationView } from '../api/client';
import type { Intent } from './lifecycleIntentStorage';
import { CharacterAvatar } from '../character/CharacterAvatar';
import { DocsLink } from '../services/DocsLink';
import { AgentProvisioningProgress } from './AgentProvisioningProgress';
import { orderedProvisioningEvents } from './agentProvisioning';

export function TaskCreationStatus({
  intent,
  operation,
  failure,
  pending,
  preparing,
  resolvedCommit,
  recovery,
}: {
  readonly intent?: Intent;
  readonly operation?: AgentLifecycleOperationView;
  readonly failure?: string;
  readonly pending: boolean;
  readonly preparing: boolean;
  readonly resolvedCommit?: string;
  readonly recovery?: ReactNode;
}) {
  if (!intent) return null;
  const status = operation?.status ?? intent.receipt?.status;
  const last = orderedProvisioningEvents(operation)
    .filter((event) => Number.isFinite(Date.parse(event.occurred_at)))
    .at(-1);
  const confirmed =
    operation?.status === 'failed'
      ? 'Preparation failed'
      : operation?.status === 'succeeded'
        ? 'Preparation completed'
        : operation?.status === 'waiting_for_connection'
          ? 'Waiting for connection'
          : operation
            ? 'Preparing'
            : 'Request accepted';
  return (
    <div className="task-creation">
      <section className="task-submitted" aria-label="Submitted task">
        <h2>Submitted task</h2>
        <p className="task-submitted__text">{intent.body.initial_message}</p>
        {intent.body.repository ? (
          <>
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
          </>
        ) : null}
      </section>
      {intent.receipt ? (
        <section className="task-creation__status" aria-label="Task preparation">
          <div className="task-creation__heading">
            <CharacterAvatar pose={status === 'failed' ? 'failed' : 'preparing'} size={72} still />
            <div>
              <h2>
                {failure
                  ? 'The latest setup status is unconfirmed'
                  : status === 'failed'
                    ? 'Agent preparation needs attention'
                    : status === 'succeeded'
                      ? 'Verifying access to your task'
                      : 'Preparing your Agent'}
              </h2>
              {failure ? (
                <p>
                  Last confirmed: {confirmed}
                  {last ? (
                    <>
                      {' '}
                      ·{' '}
                      <time dateTime={last.occurred_at}>
                        {new Date(last.occurred_at).toLocaleString()}
                      </time>
                    </>
                  ) : null}
                  .
                </p>
              ) : (
                <p>
                  {status === 'failed'
                    ? 'Task accepted, but Agent preparation failed.'
                    : status === 'waiting_for_connection'
                      ? 'Task accepted. A connection check is required.'
                      : status === 'succeeded'
                        ? 'Agent prepared. Verifying access to your task.'
                        : 'Your request was accepted. Preparation is still in progress.'}
                </p>
              )}
              {!failure && last ? (
                <p>
                  Last reported update:{' '}
                  <time dateTime={last.occurred_at}>
                    {new Date(last.occurred_at).toLocaleString()}
                  </time>
                </p>
              ) : null}
            </div>
          </div>
          {status !== 'failed' ? (
            <p>Task progress and results appear in the conversation.</p>
          ) : null}
          {failure ? <p role="alert">{failure}</p> : null}
          {recovery}
          <details className="task-creation__details">
            <summary>Setup details{failure ? ' · last confirmed' : ''}</summary>
            <AgentProvisioningProgress
              receipt={intent.receipt}
              operation={operation}
              announce={!preparing && !failure}
              presentation="task"
            />
          </details>
        </section>
      ) : (
        <section className="task-creation__status" aria-label="Task request recovery">
          <h2>{pending ? 'Sending your task request' : 'Task request needs attention'}</h2>
          {failure ? <p role="alert">{failure}</p> : null}
          {recovery}
          {!pending ? (
            <DocsLink view="recover-and-continue">Help recovering this request</DocsLink>
          ) : null}
        </section>
      )}
    </div>
  );
}
