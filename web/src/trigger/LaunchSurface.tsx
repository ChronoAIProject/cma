import { useEffect, useRef, useState } from 'react';
import { cancelLaunch, createNewLaunch, recoverInitialPrompt } from '../api/client';
import type { AppRouter } from '../routing/routes';
import { routeClick, routePath } from '../routing/routes';
import { Button } from '../ui/Button';
import { Modal } from '../ui/Modal';
import { RiskConfirmation } from '../ui/RiskConfirmation';
import {
  IconBranchOutline16,
  IconCheckOutline16,
  IconCloseOutline16,
  IconLinkOutline16,
  IconLoadingOutline16,
  IconQueueOutline14,
  IconRefreshOutline16,
  IconRightUpOutline16,
  IconWarningOutline16,
} from '../ui/icons';
import {
  deliveryState,
  isTerminalLaunch,
  mergeabilityLabel,
  reachState,
  recoverablePromptDelivery,
  sessionId,
  shortRevision,
  stateLabel,
  statusLabel,
  stepLabel,
  timestamp,
  upstreamRepository,
  workspaceCheckpoint,
} from './model';
import { useLaunchResource } from './useLaunchResource';
import { useRetryDeadline } from './useRetryDeadline';
import {
  commandOutcomeIsUnresolved,
  retainedCommandRequest,
  useIdempotentCommand,
} from './useIdempotentCommand';
import {
  commandObject,
  exactString,
  nonEmptyString,
  nonNegativeInteger,
} from './commandValidation';
import './LaunchSurface.css';

interface LaunchSurfaceProps {
  readonly ownerId: string;
  readonly launchId: string;
  readonly navigate: AppRouter['navigate'];
}

function StepIcon({ status }: { readonly status: string }) {
  switch (status) {
    case 'completed':
      return <IconCheckOutline16 />;
    case 'running':
      return <IconLoadingOutline16 />;
    case 'retryable':
      return <IconRefreshOutline16 />;
    case 'blocked':
    case 'failed':
      return <IconWarningOutline16 />;
    case 'cancelled':
      return <IconCloseOutline16 />;
    default:
      return <IconQueueOutline14 />;
  }
}

function checksLabel(value: string): string {
  return value === 'awaiting_maintainer_approval'
    ? 'Awaiting maintainer approval'
    : stateLabel(value);
}

interface LaunchRevisionDigestRequest {
  readonly launchId: string;
  readonly expectedRevision: number;
  readonly snapshotDigest: string;
}

function launchRevisionDigestRequest(
  value: unknown,
  launchId: string,
): LaunchRevisionDigestRequest | null {
  return commandObject<LaunchRevisionDigestRequest>(value, {
    launchId: exactString(launchId),
    expectedRevision: nonNegativeInteger,
    snapshotDigest: nonEmptyString,
  });
}

export function LaunchSurface({ ownerId, launchId, navigate }: LaunchSurfaceProps) {
  const resource = useLaunchResource(launchId);
  const [confirmingCancel, setConfirmingCancel] = useState(false);
  const [confirmingNew, setConfirmingNew] = useState(false);
  const [confirmingRecovery, setConfirmingRecovery] = useState(false);
  const [busy, setBusy] = useState<'cancel' | 'new' | 'recover' | null>(null);
  const [actionFailure, setActionFailure] = useState<string | null>(null);
  const [recoveryFailure, setRecoveryFailure] = useState<string | null>(null);
  const heading = useRef<HTMLHeadingElement>(null);
  const commands = useIdempotentCommand(ownerId, [`launch:${launchId}`]);
  const retryBlocked = useRetryDeadline(resource.nextRetryAt);

  useEffect(() => {
    heading.current?.focus();
  }, [launchId, resource.loading]);

  async function cancel(): Promise<void> {
    const current = resource.launch;
    if (current === null) return;
    setBusy('cancel');
    setActionFailure(null);
    try {
      const identity = {
        slot: `launch:cancel:${current.id}`,
        resource: `launch:${current.id}`,
        action: 'cancel',
      } as const;
      const request = retainedCommandRequest(
        commands.pending,
        identity,
        {
          launchId: current.id,
          expectedRevision: current.revision_number,
          snapshotDigest: current.effective.digest,
        },
        (value) => launchRevisionDigestRequest(value, current.id),
      );
      const next = await commands.execute(
        { ...identity, request, persistRequest: true },
        'Launch cancellation',
        (idempotencyKey, frozen) =>
          cancelLaunch(frozen.launchId, frozen.expectedRevision, frozen.snapshotDigest, {
            idempotencyKey,
          }),
      );
      resource.accept(next);
      setConfirmingCancel(false);
    } catch (cause) {
      if (commandOutcomeIsUnresolved(cause)) {
        setActionFailure(
          'The cancellation outcome is unknown. CMA retained the exact command; reconcile it before another Launch action.',
        );
        await resource.refresh();
      } else {
        setConfirmingCancel(false);
        setActionFailure(cause instanceof Error ? cause.message : String(cause));
      }
    } finally {
      setBusy(null);
    }
  }

  async function createAnother(): Promise<void> {
    const current = resource.launch;
    if (current === null) return;
    setBusy('new');
    setActionFailure(null);
    try {
      const identity = {
        slot: `launch:new:${current.id}`,
        resource: `launch:${current.id}`,
        action: 'new-launch',
      } as const;
      const request = retainedCommandRequest(
        commands.pending,
        identity,
        {
          publicId: current.trigger_public_id,
        },
        (value) => commandObject(value, { publicId: exactString(current.trigger_public_id) }),
      );
      const next = await commands.execute(
        { ...identity, request, persistRequest: true },
        'New Launch creation',
        (idempotencyKey, frozen) => createNewLaunch(frozen.publicId, { idempotencyKey }),
      );
      setConfirmingNew(false);
      navigate({ kind: 'trigger', publicId: next.trigger_public_id });
    } catch (cause) {
      if (commandOutcomeIsUnresolved(cause)) {
        setActionFailure(
          'The New Launch outcome is unknown. CMA retained the exact command; reconcile it before another Launch action.',
        );
        await resource.refresh();
      } else {
        setConfirmingNew(false);
        setActionFailure(cause instanceof Error ? cause.message : String(cause));
      }
    } finally {
      setBusy(null);
    }
  }

  async function recoverPrompt(): Promise<void> {
    const current = resource.launch;
    if (current === null) return;
    const slot = `launch:recover-prompt:${current.id}`;
    const pending = commands.pending?.slot === slot;
    const delivery = recoverablePromptDelivery(current);
    if (!pending && delivery === undefined) {
      setRecoveryFailure('This Launch no longer requires initial-task recovery.');
      await resource.refresh();
      return;
    }

    setBusy('recover');
    setRecoveryFailure(null);
    try {
      const identity = {
        slot,
        resource: `launch:${current.id}`,
        action: 'recover-initial-prompt',
      } as const;
      const request = retainedCommandRequest(
        commands.pending,
        identity,
        {
          launchId: current.id,
          expectedRevision: current.revision_number,
          payloadHash: delivery?.payload_hash ?? '',
        },
        (value) =>
          commandObject(value, {
            launchId: exactString(current.id),
            expectedRevision: nonNegativeInteger,
            payloadHash: nonEmptyString,
          }),
      );
      if (request.payloadHash === '') return;
      const next = await commands.execute(
        { ...identity, request, persistRequest: true },
        'initial-task recovery',
        (idempotencyKey, frozen) =>
          recoverInitialPrompt(frozen.launchId, frozen.expectedRevision, frozen.payloadHash, {
            idempotencyKey,
          }),
      );
      resource.accept(next);
      setConfirmingRecovery(false);
    } catch (cause) {
      setRecoveryFailure(
        commandOutcomeIsUnresolved(cause)
          ? 'The recovery outcome is unknown. CMA retained the exact authorization; reconcile it before another Launch action.'
          : cause instanceof Error
            ? cause.message
            : String(cause),
      );
      await resource.refresh();
    } finally {
      setBusy(null);
    }
  }

  if (resource.loading && resource.launch === null) {
    return (
      <main className="trigger-workflow" aria-busy="true">
        <header className="trigger-workflow__head">
          <h1 ref={heading} tabIndex={-1}>
            Launch
          </h1>
        </header>
        <div className="trigger-workflow__loading">
          <div className="skeleton" />
          <div className="skeleton" />
          <div className="skeleton" />
        </div>
      </main>
    );
  }

  if (resource.launch === null) {
    return (
      <main className="trigger-workflow">
        <section className="trigger-workflow__fatal" role="alert">
          <IconWarningOutline16 />
          <div>
            <h1 ref={heading} tabIndex={-1}>
              Launch unavailable
            </h1>
            <p>{resource.failure ?? 'This Launch could not be read.'}</p>
            {resource.failureDisposition === 'retryable' ? (
              <Button
                variant="outline"
                icon={<IconRefreshOutline16 />}
                disabled={retryBlocked}
                onClick={() => void resource.refresh()}
              >
                Retry Launch
              </Button>
            ) : null}
          </div>
        </section>
      </main>
    );
  }

  const launch = resource.launch;
  const repository = upstreamRepository(launch);
  const linkedSession = sessionId(launch);
  const checkpoint = workspaceCheckpoint(launch);
  const reach = reachState(launch);
  const delivery = deliveryState(launch);
  const terminal = isTerminalLaunch(launch);
  const pendingCancel = commands.pending?.slot === `launch:cancel:${launch.id}`;
  const pendingNew = commands.pending?.slot === `launch:new:${launch.id}`;
  const pendingRecovery = commands.pending?.slot === `launch:recover-prompt:${launch.id}`;
  const recoveryAvailable = recoverablePromptDelivery(launch) !== undefined || pendingRecovery;
  const recoveryStatus =
    busy === 'recover'
      ? pendingRecovery
        ? 'Reconciling the retained recovery authorization.'
        : 'Submitting the recovery authorization.'
      : pendingRecovery
        ? 'CMA has not returned a finite result. Only the retained recovery authorization can be reconciled.'
        : undefined;
  const contributionRoute = { kind: 'contribute', launchId: launch.id } as const;
  const authenticationRoute =
    launch.state === 'auth_required'
      ? ({ kind: 'trigger_recovery', launchId: launch.id } as const)
      : undefined;
  const sessionRoute =
    linkedSession === undefined
      ? undefined
      : ({ kind: 'session', sessionId: linkedSession } as const);

  return (
    <main className="trigger-workflow launch">
      <header className="trigger-workflow__head launch__head">
        <div>
          <p className="eyebrow">Launch</p>
          <h1 ref={heading} tabIndex={-1}>
            {repository.canonical_owner}/{repository.canonical_name}
          </h1>
          <p className="launch__state">{stateLabel(launch.state)}</p>
        </div>
        <code title={launch.id}>{shortRevision(launch.id)}</code>
      </header>

      {resource.offline ? (
        <div className="launch__offline" role="status">
          Offline · retained Launch data · Last updated{' '}
          {resource.lastUpdated === null
            ? 'unknown'
            : new Date(resource.lastUpdated).toLocaleTimeString()}
        </div>
      ) : null}
      {!resource.offline && resource.failure !== null ? (
        <div className="notice notice--fail trigger-workflow__notice" role="alert">
          <IconWarningOutline16 />
          <div>
            <p className="notice__title">Launch data may be stale.</p>
            <p className="notice__detail">{resource.failure}</p>
            {resource.nextRetryAt === null ? null : (
              <p className="notice__detail">
                Automatic retry after {new Date(resource.nextRetryAt).toLocaleTimeString()}.
              </p>
            )}
            {resource.failureDisposition === 'retryable' ? (
              <Button
                size="sm"
                variant="outline"
                icon={<IconRefreshOutline16 />}
                disabled={retryBlocked}
                onClick={() => void resource.refresh()}
              >
                Retry refresh
              </Button>
            ) : null}
          </div>
        </div>
      ) : null}
      {commands.failure === null ? null : (
        <div className="notice notice--fail trigger-workflow__notice" role="alert">
          <IconWarningOutline16 />
          <div>
            <p className="notice__title">Command recovery is unavailable.</p>
            <p className="notice__detail">{commands.failure}</p>
          </div>
        </div>
      )}
      <p className="launch__live" aria-live="polite" aria-atomic="true">
        {commands.pending === null
          ? resource.liveSummary
          : `Reconciling ${commands.pending.label.toLowerCase()}`}
      </p>

      {actionFailure === null ? null : (
        <div className="notice notice--fail trigger-workflow__notice" role="alert">
          <IconWarningOutline16 />
          <div>
            <p className="notice__title">The Launch action did not complete.</p>
            <p className="notice__detail">{actionFailure}</p>
          </div>
        </div>
      )}

      <div className="trigger-workflow__scroll launch__scroll">
        <section className="launch-band" aria-labelledby="launch-repository-heading">
          <div className="launch-band__heading">
            <IconBranchOutline16 />
            <h2 id="launch-repository-heading">Repository</h2>
          </div>
          <dl className="launch-facts">
            <div>
              <dt>Work label</dt>
              <dd>{launch.effective.work_label}</dd>
            </div>
            <div>
              <dt>Upstream</dt>
              <dd>
                {repository.canonical_owner}/{repository.canonical_name}
              </dd>
            </div>
            <div>
              <dt>Base</dt>
              <dd>
                <code>{repository.selected_ref}</code> at{' '}
                <code title={launch.source_sha}>{shortRevision(launch.source_sha)}</code>
              </dd>
            </div>
            <div>
              <dt>GitHub account</dt>
              <dd>{launch.connection?.account_login ?? 'Not connected'}</dd>
            </div>
            <div>
              <dt>Fork</dt>
              <dd>
                {launch.fork === null ? (
                  'Not created'
                ) : (
                  <a href={launch.fork.canonical_url} target="_blank" rel="noopener noreferrer">
                    {launch.fork.repository.canonical_owner}/{launch.fork.repository.canonical_name}
                    <IconRightUpOutline16 />
                  </a>
                )}
              </dd>
            </div>
            <div>
              <dt>Launch branch</dt>
              <dd>
                {launch.branch === null ? (
                  'Not created'
                ) : (
                  <a href={launch.branch.canonical_url} target="_blank" rel="noopener noreferrer">
                    <code>{launch.branch.name}</code>
                    <IconRightUpOutline16 />
                  </a>
                )}
              </dd>
            </div>
            <div>
              <dt>Branch head</dt>
              <dd>
                {launch.branch === null ? (
                  'Not created'
                ) : (
                  <code title={launch.branch.current_head}>
                    {shortRevision(launch.branch.current_head)}
                  </code>
                )}
              </dd>
            </div>
            <div>
              <dt>Workspace checkpoint</dt>
              <dd>
                {checkpoint === undefined ? (
                  'Not captured'
                ) : (
                  <code>{shortRevision(checkpoint)}</code>
                )}
              </dd>
            </div>
            <div>
              <dt>GitHub reach</dt>
              <dd>{reach === undefined ? 'Not provisioned' : stateLabel(reach)}</dd>
            </div>
            <div>
              <dt>Initial task</dt>
              <dd>{delivery === undefined ? 'Not dispatched' : stateLabel(delivery)}</dd>
            </div>
          </dl>
        </section>

        <section className="launch-band" aria-labelledby="launch-progress-heading">
          <div className="launch-band__heading">
            <IconQueueOutline14 />
            <h2 id="launch-progress-heading">Automation progress</h2>
          </div>
          <ol className="launch-progress">
            {[...launch.steps]
              .sort((left, right) => left.sequence - right.sequence)
              .map((step) => (
                <li key={step.key} data-status={step.status}>
                  <span className="launch-progress__icon" aria-hidden="true">
                    <StepIcon status={step.status} />
                  </span>
                  <div className="launch-progress__body">
                    <div className="launch-progress__line">
                      <strong>{stepLabel(step.key)}</strong>
                      <span className="launch-progress__status">{statusLabel(step.status)}</span>
                      <time dateTime={step.completed_at ?? step.started_at ?? undefined}>
                        {timestamp(step.completed_at ?? step.started_at)}
                      </time>
                    </div>
                    {step.safe_detail === '' ? null : <p>{step.safe_detail}</p>}
                    {step.remediation === '' ? null : (
                      <p className="launch-progress__remediation">{step.remediation}</p>
                    )}
                    {step.status === 'retryable' ? (
                      <p className="launch-progress__retry">
                        CMA reconciles the previous outcome before its bounded retry.
                      </p>
                    ) : null}
                  </div>
                </li>
              ))}
          </ol>
          {launch.blocked_reason === null ? null : (
            <div className="notice notice--quiet launch__blocked" role="status">
              <IconWarningOutline16 />
              <div>
                <p className="notice__title">{stateLabel(launch.blocked_reason)}</p>
                <p className="notice__detail">This Launch remains resumable.</p>
                {authenticationRoute === undefined ? null : (
                  <a
                    className="button button--outline button--sm"
                    href={routePath(authenticationRoute)}
                    onClick={(event) => routeClick(event, authenticationRoute, navigate)}
                  >
                    Resume GitHub access
                  </a>
                )}
              </div>
            </div>
          )}
          {launch.failure === null ? null : (
            <div className="notice notice--fail launch__failure" role="alert">
              <IconWarningOutline16 />
              <div>
                <p className="notice__title">{launch.failure.detail}</p>
                <p className="notice__detail">{launch.failure.remediation}</p>
                <code>
                  {launch.failure.code} · {launch.failure.correlation_id}
                </code>
              </div>
            </div>
          )}
        </section>

        <section className="launch-band" aria-labelledby="launch-session-heading">
          <div className="launch-band__heading">
            <IconLinkOutline16 />
            <h2 id="launch-session-heading">Session handoff</h2>
          </div>
          {sessionRoute === undefined ? (
            <p className="launch-band__empty">The session link appears after provisioning.</p>
          ) : (
            <a
              className="launch-route"
              href={routePath(sessionRoute)}
              onClick={(event) => routeClick(event, sessionRoute, navigate)}
            >
              <span>
                <strong>Open CMA session</strong>
                <small>{linkedSession}</small>
              </span>
              <IconLinkOutline16 />
            </a>
          )}
        </section>

        <section className="launch-band" aria-labelledby="launch-contribution-heading">
          <div className="launch-band__heading">
            <IconBranchOutline16 />
            <h2 id="launch-contribution-heading">Contribution</h2>
          </div>
          <dl className="launch-facts">
            <div>
              <dt>Diff</dt>
              <dd>
                {launch.diff === null
                  ? 'Not reviewed'
                  : `${String(launch.diff.total_changed_files)} changed files`}
              </dd>
            </div>
            <div>
              <dt>Branch publication</dt>
              <dd>
                {launch.contribution === null
                  ? 'Not published'
                  : stateLabel(launch.contribution.state)}
              </dd>
            </div>
            <div>
              <dt>Pull request</dt>
              <dd>
                {launch.pull_request === null ? (
                  'Not opened'
                ) : launch.pull_request.canonical_url === null ? (
                  stateLabel(launch.pull_request.state)
                ) : (
                  <a
                    href={launch.pull_request.canonical_url}
                    target="_blank"
                    rel="noopener noreferrer"
                  >
                    #{String(launch.pull_request.number)} · {stateLabel(launch.pull_request.state)}
                    <IconRightUpOutline16 />
                  </a>
                )}
              </dd>
            </div>
            {launch.pull_request === null ? null : (
              <>
                <div>
                  <dt>Mergeability</dt>
                  <dd>{mergeabilityLabel(launch.pull_request.mergeability)}</dd>
                </div>
                <div>
                  <dt>Checks</dt>
                  <dd>{checksLabel(launch.pull_request.checks)}</dd>
                </div>
              </>
            )}
          </dl>
          {linkedSession === undefined ? (
            <p className="control-reason">
              Contribution review becomes available after the session is ready.
            </p>
          ) : (
            <a
              className="button button--outline button--md launch__contribute"
              href={routePath(contributionRoute)}
              onClick={(event) => routeClick(event, contributionRoute, navigate)}
            >
              Review contribution
            </a>
          )}
        </section>
      </div>

      <footer className="trigger-workflow__actions">
        <span className="control-reason">Updated {timestamp(launch.updated_at)}</span>
        <Button
          icon={<IconRefreshOutline16 />}
          disabled={busy !== null}
          onClick={() => void resource.refresh()}
        >
          Refresh
        </Button>
        {recoveryAvailable ? (
          <Button
            variant="primary"
            icon={<IconRefreshOutline16 />}
            disabled={busy !== null || commands.pending !== null}
            onClick={() => {
              setRecoveryFailure(null);
              setConfirmingRecovery(true);
            }}
          >
            Recover initial task
          </Button>
        ) : null}
        <Button
          variant="outline"
          disabled={busy !== null || commands.pending !== null}
          onClick={() => setConfirmingNew(true)}
        >
          New Launch
        </Button>
        {terminal ? null : (
          <Button
            variant="danger"
            disabled={busy !== null || commands.pending !== null}
            onClick={() => setConfirmingCancel(true)}
          >
            Cancel Launch
          </Button>
        )}
      </footer>

      <Modal
        open={confirmingCancel || pendingCancel}
        title="Cancel this Launch?"
        description="CMA will stop new orchestration and revoke Launch reach where safe. Your fork, branch, commits, pull request, session history, and workspace record are not deleted."
        onClose={() => {
          if (busy === null && !pendingCancel) setConfirmingCancel(false);
        }}
        footer={
          <>
            <Button
              disabled={busy !== null || pendingCancel}
              onClick={() => setConfirmingCancel(false)}
            >
              Keep Launch
            </Button>
            <Button
              variant="danger"
              disabled={busy !== null || (commands.pending !== null && !pendingCancel)}
              onClick={() => void cancel()}
            >
              {busy === 'cancel'
                ? pendingCancel
                  ? 'Reconciling'
                  : 'Submitting'
                : pendingCancel
                  ? 'Reconcile cancellation'
                  : 'Cancel Launch'}
            </Button>
          </>
        }
      />
      <RiskConfirmation
        open={confirmingRecovery || pendingRecovery}
        title="Authorize another initial task?"
        description="CMA cannot determine whether the previous initial task started. Authorizing another dispatch may execute the task twice."
        acknowledgeLabel="I understand that the initial task may execute twice."
        confirmLabel={
          busy === 'recover'
            ? pendingRecovery
              ? 'Reconciling'
              : 'Submitting'
            : pendingRecovery
              ? 'Reconcile recovery'
              : 'Authorize another dispatch'
        }
        busy={busy === 'recover'}
        dismissible={busy !== 'recover' && !pendingRecovery}
        failure={recoveryFailure ?? undefined}
        status={recoveryStatus}
        onCancel={() => {
          setConfirmingRecovery(false);
          setRecoveryFailure(null);
        }}
        onConfirm={() => void recoverPrompt()}
      />
      <Modal
        open={confirmingNew || pendingNew}
        title="Create a separate Launch?"
        description="This creates a new Launch-owned branch and may allocate another sandbox and session. The current Launch, session, branch, and pull request stay unchanged."
        onClose={() => {
          if (busy === null && !pendingNew) setConfirmingNew(false);
        }}
        footer={
          <>
            <Button disabled={busy !== null || pendingNew} onClick={() => setConfirmingNew(false)}>
              Cancel
            </Button>
            <Button
              variant="primary"
              disabled={busy !== null || (commands.pending !== null && !pendingNew)}
              onClick={() => void createAnother()}
            >
              {busy === 'new'
                ? pendingNew
                  ? 'Reconciling'
                  : 'Submitting'
                : pendingNew
                  ? 'Reconcile New Launch'
                  : 'Create New Launch'}
            </Button>
          </>
        }
      />
    </main>
  );
}
