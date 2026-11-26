import { useState } from 'react';
import { deletionTarget, type DeletionObservation, type DeletionRequest } from './deletion';
import { routePath } from './routing/routes';
import { ProvisioningProgress } from './sandbox/ProvisioningProgress';
import { LiveActivity } from './sandbox/LiveActivity';
import { activityEntries } from './sandbox/activity';
import { Button } from './ui/Button';
import { IconCheckOutline16, IconCopyOutline16, IconWarningOutline16 } from './ui/icons';
import './DeletionPresentation.css';
import { stoppedProofMissing, deleteMayHaveBeenSent } from './agent/stopDeletion';

export function DeletionReference({ request }: { readonly request: DeletionRequest }) {
  const [copy, setCopy] = useState<'copied' | 'failed'>();
  return (
    <div className="deletion-reference">
      <span>Request reference</span>
      <code>{request.key}</code>
      <Button
        size="sm"
        icon={<IconCopyOutline16 />}
        onClick={() => {
          void Promise.resolve()
            .then(() => navigator.clipboard.writeText(request.key))
            .then(
              () => setCopy('copied'),
              () => setCopy('failed'),
            );
        }}
      >
        Copy reference
      </Button>
      {copy === undefined ? null : (
        <p role="status">
          {copy === 'copied'
            ? 'Reference copied.'
            : 'Copy unavailable. Select the full reference above.'}
        </p>
      )}
    </div>
  );
}

export function DeletionStatusPanel({
  request,
  observation,
  complete = observation?.complete === true,
  sending,
  readError,
  commandError,
}: {
  readonly request: DeletionRequest;
  readonly observation?: DeletionObservation;
  readonly complete?: boolean;
  readonly sending: boolean;
  readonly readError?: string;
  readonly commandError?: string;
}) {
  const target = deletionTarget(request.action);
  const unknownDelete = !request.accepted && deleteMayHaveBeenSent(request);
  const blocked = request.stop?.phase === 'blocked' && !request.accepted;
  const stopping =
    request.stop !== undefined &&
    !request.accepted &&
    !unknownDelete &&
    observation?.agent?.status !== 'stopped';
  const state = complete
    ? 'Deleted'
    : blocked
      ? 'Needs attention'
      : readError
        ? 'Status unavailable'
        : sending
          ? 'Submitting'
          : observation?.failure
            ? 'Cleanup needs attention'
            : observation?.retirementDelayed
              ? 'Cleanup taking longer'
              : observation?.inProgress
                ? 'Deleting'
                : request.accepted
                  ? 'Deletion requested'
                  : !unknownDelete && request.stop?.phase === 'accepted'
                    ? 'Stopping'
                    : 'Request unconfirmed';
  return (
    <section className="deletion-status" aria-label="Deletion status">
      <span className="deletion-status__badge" data-complete={complete}>
        {complete ? <IconCheckOutline16 /> : <IconWarningOutline16 />}
        {state}
      </span>
      <p role="status">
        {complete
          ? `${target} deletion is confirmed.`
          : unknownDelete
            ? 'Delete may already have started. Its result is unconfirmed; keep checking the original request.'
            : blocked
              ? stoppedProofMissing
              : stopping
                ? 'Waiting for the Agent to stop. Deletion will follow only after stopping is confirmed.'
                : sending
                  ? 'Submitting delete request.'
                  : request.accepted
                    ? 'Delete request accepted. Cleanup continues even if you close this page.'
                    : 'The delete request has not been confirmed. Check the same request before trying another action.'}
      </p>
      {!complete && !request.accepted ? (
        <p>
          {blocked && unknownDelete
            ? 'End this request stops automatic steps and keeps this deletion available for status checks and allowed retries.'
            : blocked
              ? 'End this request to stop further steps. Commands already sent may still finish. Deleting later requires a new confirmation.'
              : stopping
                ? 'Stop and delete keeps the original requests. Close preserves them; reopen to check or continue.'
                : 'Check request retries the original delete request with the same reference.'}
        </p>
      ) : null}
      {target === 'Agent' ? (
        <p>
          {complete
            ? 'Agent deleted. Its Sandbox environment is retained and still counts toward your environment limit.'
            : 'Only this Agent and its Worktree are being removed. Its Sandbox environment remains.'}{' '}
          {observation?.sandboxId ? (
            <a href={routePath({ kind: 'sandbox', sandboxId: observation.sandboxId })}>
              Review parent environment
            </a>
          ) : null}
        </p>
      ) : target === 'Sandbox' && request.accepted && !complete ? (
        <p>
          Deleting — still uses one Sandbox slot. The slot becomes available when deletion is
          confirmed.
        </p>
      ) : null}
      {!complete && !readError && observation?.retirementDelayed ? (
        <p>
          The Sandbox has exceeded its expected deletion time. Cleanup is still pending. Keep
          checking this request for confirmation.
        </p>
      ) : null}
      {complete ? (
        <p>Cleanup confirmed in this view.</p>
      ) : observation ? (
        <p>
          Last status read:{' '}
          <time dateTime={observation.observedAt}>
            {new Date(observation.observedAt).toLocaleString()}
          </time>
        </p>
      ) : null}
      {observation?.failure === undefined || complete ? null : (
        <p role="alert">
          Cleanup reported: {observation.failure.detail}. Deletion is not complete.
        </p>
      )}
      {complete ||
      commandError === undefined ||
      (blocked && commandError === stoppedProofMissing) ? null : (
        <p role="alert">{commandError}</p>
      )}
      {readError === undefined || complete ? null : <p role="alert">{readError}</p>}
      <DeletionReference key={request.key} request={request} />
      <details className="deletion-status__details">
        <summary>Cleanup progress</summary>
        {observation === undefined || complete ? null : (
          <ProvisioningProgress title="Deletion progress" steps={observation.steps} />
        )}
        <LiveActivity
          entries={activityEntries(
            observation?.agentEvents ?? observation?.events ?? [],
            request.action.kind === 'delete_agent' ? 'agent' : 'sandbox',
          )}
          syncedAt={observation?.observedAt}
          stale={!complete && readError !== undefined}
          loading={!complete && observation === undefined && readError === undefined}
        />
      </details>
    </section>
  );
}
