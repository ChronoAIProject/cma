import { useId } from 'react';
import type { CmaWebClient } from '../api/client';
import type { SandboxRuntimeUpdate as RuntimeUpdate } from '../api/types';
import { LandingCharacter } from '../landing/LandingCharacter';
import { Button } from '../ui/Button';
import { Presence } from '../ui/Presence';
import { useRuntimeUpdate } from './useRuntimeUpdate';
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
    detail: 'The update could not finish. Review the reported reason and retry when available.',
  },
} as const;

export function SandboxRuntimeUpdate({
  client,
  sandboxId,
  ownerId,
  revision,
  update,
  stale,
  busy,
  onAccepted,
}: {
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
  const copy = statusCopy[state];
  const title = submitting ? 'Requesting runtime update' : copy.title;
  const complete = state === 'up_to_date' && current.completedAt !== undefined;
  const animate = submitting || state === 'updating';
  const message = !submitting ? command.intent?.message : undefined;
  const target = unresolved ? command.intent?.target : current.targetVersion;
  const unavailable = client.updateSandboxRuntime === undefined || stale || busy || submitting;

  return (
    <section className="sandbox-page__band" aria-label="Runtime">
      <h2>Runtime</h2>
      <dl>
        <div>
          <dt>Current runtime</dt>
          <dd>
            {current.currentVersion === null ? (
              'Not reported'
            ) : (
              <code>{current.currentVersion}</code>
            )}
          </dd>
        </div>
        <div>
          <dt>Latest runtime</dt>
          <dd>
            <code>{current.latestVersion}</code>
          </dd>
        </div>
        {target !== undefined && target !== current.latestVersion ? (
          <div>
            <dt>Requested runtime</dt>
            <dd>
              <code>{target}</code>
            </dd>
          </div>
        ) : null}
        <div>
          <dt>Automatic updates</dt>
          <dd>
            Daily at {current.schedule.localTime} SGT ({current.schedule.timezone}), when idle
          </dd>
        </div>
      </dl>
      <div className="sandbox-recovery" data-state={submitting ? 'requesting' : state}>
        <div className="sandbox-recovery__character" aria-hidden="true">
          <LandingCharacter
            compact
            state={
              animate
                ? 'running'
                : state === 'queued'
                  ? 'pending'
                  : state === 'failed'
                    ? 'failed'
                    : state === 'up_to_date'
                      ? 'completed'
                      : 'brand'
            }
          />
        </div>
        <div className="sandbox-recovery__content">
          <p className="sandbox-recovery__title" role="status">
            {complete && !submitting ? 'Runtime updated' : title}
          </p>
          <p key={title} className="sandbox-recovery__stage">
            {submitting ? 'Saving your update request.' : copy.detail}
          </p>
          {current.preservesHome ? (
            <p>Your existing home disk, files and conversations are preserved.</p>
          ) : null}
          <Presence>
            {state === 'failed' && current.failure ? (
              <p role="alert">{current.failure.message}</p>
            ) : null}
          </Presence>
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
      {current.blockedReason ? <p id={reasonId}>{current.blockedReason}</p> : null}
      {unresolved ? (
        <Button disabled={unavailable} onClick={command.retry}>
          {submitting ? 'Requesting update…' : 'Retry update request'}
        </Button>
      ) : state !== 'up_to_date' ? (
        <Button
          disabled={unavailable || !current.canUpdate}
          aria-describedby={current.blockedReason ? reasonId : undefined}
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
