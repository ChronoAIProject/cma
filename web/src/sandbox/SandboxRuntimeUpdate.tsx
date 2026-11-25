import { useId } from 'react';
import type { CmaWebClient } from '../api/client';
import type { SandboxDetail, SandboxRuntimeUpdate as RuntimeUpdate } from '../api/types';
import { StateDot } from '../ui/StateDot';
import { Button } from '../ui/Button';
import { Presence } from '../ui/Presence';
import { useRuntimeUpdate } from './useRuntimeUpdate';
import { SandboxRuntimeFacts, runtimeLifecycleNotice } from './SandboxRuntimeFacts';
import './SandboxRecoveryNotice.css';

const statusCopy = {
  up_to_date: { title: 'Runtime up to date', detail: 'This Sandbox uses the latest runtime.' },
  available: {
    title: 'Runtime update available',
    detail: 'Update now, or let the daily update run when your Sandbox is idle.',
  },
  queued: {
    title: 'Runtime update queued',
    detail:
      'Waiting for current Agent turns, work and approvals to finish. Your Agents can continue working.',
  },
  updating: {
    title: 'Updating your runtime',
    detail:
      'The Sandbox will be ready when the update finishes. Conversation history remains available.',
  },
  failed: {
    title: 'Runtime update needs attention',
    detail: 'The runtime update stopped at its current step.',
  },
} as const;

const legacyFailureMessage =
  'The runtime update paused safely. Your home and conversations are retained. Retry the update to continue.';

function failedUpdateMessage(update: RuntimeUpdate, canRetry: boolean): string {
  const detail = statusCopy.failed.detail;
  if (update.canUpdate && !canRetry) return `${detail} Retry is unavailable in this client.`;
  const reported = update.failure?.message;
  if (reported && reported !== legacyFailureMessage) return reported;
  return canRetry
    ? `${detail} Retry the runtime update to continue.`
    : `${detail} Contact the deployment operator for recovery help.`;
}

export function SandboxRuntimeUpdate({
  client,
  sandboxId,
  ownerId,
  revision,
  update,
  stale,
  busy,
  onAccepted,
  sandboxStatus,
  technicalDetails = 'inline',
}: {
  readonly sandboxStatus?: SandboxDetail['status'];
  readonly technicalDetails?: 'inline' | 'disclosure';
  readonly client: CmaWebClient;
  readonly sandboxId: string;
  readonly ownerId: string;
  readonly revision: number;
  readonly update: RuntimeUpdate;
  readonly stale: boolean;
  readonly busy: boolean;
  readonly onAccepted: (receipt: {
    readonly runtimeUpdate: RuntimeUpdate;
    readonly revision: number;
  }) => void;
}) {
  const reasonId = useId();
  const command = useRuntimeUpdate(
    client,
    sandboxId,
    ownerId,
    revision,
    !stale && !busy,
    onAccepted,
  );
  const current = command.intent?.receipt?.runtimeUpdate ?? update;
  const unresolved =
    command.intent !== undefined && !command.intent.receipt && !command.intent.refused;
  const submitting = command.busy;
  const state = current.state;
  const unavailableUpdate = state === 'available' && !current.canUpdate;
  const copy = unavailableUpdate
    ? {
        title: 'Runtime update unavailable',
        detail: 'A newer managed runtime version is available.',
      }
    : statusCopy[state];
  const lifecycle = runtimeLifecycleNotice(sandboxStatus, current, stale);
  const blockedReason =
    sandboxStatus === 'failed' && unavailableUpdate
      ? 'Runtime updates are blocked by the Sandbox failure. Open Technical details for the reported failure.'
      : current.blockedReason;
  const title =
    lifecycle?.title ??
    (submitting
      ? 'Requesting runtime update'
      : unresolved
        ? 'Runtime update unconfirmed'
        : copy.title);
  const complete = !unresolved && state === 'up_to_date' && current.completedAt !== undefined;
  const animate = submitting || state === 'updating';
  const message = !submitting ? command.intent?.message : undefined;
  const target = unresolved ? command.intent?.target : current.targetVersion;
  const unavailable = client.updateSandboxRuntime === undefined || stale || busy || submitting;
  const failure =
    state === 'failed' && !unresolved && !submitting && !stale && !busy
      ? failedUpdateMessage(current, current.canUpdate && !unavailable)
      : undefined;

  return (
    <section className="sandbox-page__band" aria-label="Runtime">
      <h2>Runtime</h2>
      <div className="sandbox-recovery" data-state={submitting ? 'requesting' : state}>
        <div className="sandbox-recovery__icon" aria-hidden="true">
          <StateDot
            size={10}
            state={
              lifecycle?.tone ??
              (unresolved && !submitting
                ? 'warning'
                : animate
                  ? 'ongoing'
                  : state === 'queued'
                    ? 'neutral'
                    : state === 'failed' || (sandboxStatus === 'failed' && unavailableUpdate)
                      ? 'error'
                      : state === 'up_to_date'
                        ? 'done'
                        : 'warning')
            }
          />
        </div>
        <div className="sandbox-recovery__content">
          <p className="sandbox-recovery__title" role="status">
            {complete && !submitting && !lifecycle ? 'Runtime updated' : title}
          </p>
          {lifecycle || !failure ? (
            <p key={title} className="sandbox-recovery__stage">
              {lifecycle?.detail ??
                (submitting
                  ? 'Saving your update request.'
                  : unresolved
                    ? 'The result of your update request has not been confirmed.'
                    : copy.detail)}
            </p>
          ) : null}
          {current.preservesHome && sandboxStatus !== 'retired' && sandboxStatus !== 'retiring' ? (
            <p>Your existing home disk, files and conversations are preserved.</p>
          ) : null}
          <Presence>{failure ? <p role="alert">{failure}</p> : null}</Presence>
          {current.completedAt !== undefined ? (
            <p>
              Last updated{' '}
              <time dateTime={current.completedAt}>
                {new Date(current.completedAt).toLocaleString()}
              </time>
            </p>
          ) : null}
          {stale ? <p>Reconnecting to status updates. The last confirmed state is shown.</p> : null}
        </div>
      </div>
      <Presence>{message ? <p role="alert">{message}</p> : null}</Presence>
      {blockedReason ? <p id={reasonId}>{blockedReason}</p> : null}
      {technicalDetails === 'disclosure' ? (
        <details className="sandbox-runtime-details">
          <summary>Technical details</summary>
          <SandboxRuntimeFacts
            sandboxId={sandboxId}
            update={current}
            target={target}
            sandboxStatus={sandboxStatus}
            copyable
          />
        </details>
      ) : (
        <SandboxRuntimeFacts update={current} target={target} sandboxStatus={sandboxStatus} />
      )}
      {unresolved ? (
        <Button disabled={unavailable} onClick={command.retry}>
          {submitting ? 'Requesting update…' : 'Retry update request'}
        </Button>
      ) : state !== 'up_to_date' ? (
        <Button
          disabled={unavailable || !current.canUpdate}
          aria-describedby={blockedReason ? reasonId : undefined}
          onClick={() =>
            command.start(
              state === 'failed'
                ? (current.targetVersion ?? current.latestVersion)
                : current.latestVersion,
            )
          }
        >
          {state === 'failed'
            ? 'Retry runtime update'
            : state === 'queued'
              ? 'Update queued'
              : state === 'updating'
                ? 'Updating runtime…'
                : 'Update runtime'}
        </Button>
      ) : null}
    </section>
  );
}
