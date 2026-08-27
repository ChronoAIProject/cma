import { useEffect, useRef, useState } from 'react';
import type {
  DiffArtifact,
  DiffFile,
  DiffReview,
  DiffValidationOutcome,
  LaunchCapabilityRevocationState,
  PullRequestDraft,
  PullRequestState,
} from '@chronoai/cma-sdk';
import {
  confirmPullRequest,
  downloadDiffArtifact,
  publishLaunchBranch,
  readLaunchDiff,
  requestLaunchDiff,
  reviewPullRequest,
} from '../api/client';
import type { AppRouter } from '../routing/routes';
import { routeClick, routePath } from '../routing/routes';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { Modal } from '../ui/Modal';
import {
  IconBranchOutline16,
  IconCheckOutline16,
  IconChevronRightOutline14,
  IconDownloadOutline16,
  IconLinkOutline16,
  IconRefreshOutline16,
  IconRightUpOutline16,
  IconWarningOutline16,
} from '../ui/icons';
import {
  mergeabilityLabel,
  reachState,
  sessionId,
  shortRevision,
  stateLabel,
  timestamp,
  upstreamRepository,
} from './model';
import { useLaunchResource } from './useLaunchResource';
import {
  commandOutcomeIsUnresolved,
  retainedCommandMetadata,
  retainedCommandRequest,
  useIdempotentCommand,
} from './useIdempotentCommand';
import type { PendingBrowserCommand } from './useIdempotentCommand';
import {
  commandObject,
  exactString,
  nonEmptyString,
  nonNegativeInteger,
  oneOfLiterals,
} from './commandValidation';
import './ContributionSurface.css';

interface ContributionSurfaceProps {
  readonly ownerId: string;
  readonly launchId: string;
  readonly navigate: AppRouter['navigate'];
}

type Busy = 'capture' | 'publish' | 'proposal' | 'pr' | 'download' | null;

interface PullRequestCommandMetadata {
  readonly launchId: string;
  readonly operation: 'open' | 'update';
  readonly expectedRevision: number;
  readonly snapshotDigest: string;
}

interface PullRequestCommandRequest extends PullRequestCommandMetadata {
  readonly edits: { readonly title: string; readonly body: string };
}

interface LaunchRevisionDigestRequest {
  readonly launchId: string;
  readonly expectedRevision: number;
  readonly snapshotDigest: string;
}

type RemediationAction = 'capture' | 'recover_connection' | 'refresh_reach' | 'session';

interface ContributionRemediation {
  readonly reason: string;
  readonly action: RemediationAction;
  readonly label: string;
}

function reachRemediation(
  state: LaunchCapabilityRevocationState | undefined,
): ContributionRemediation | null {
  switch (state) {
    case 'active':
      return null;
    case 'revocation_pending':
      return {
        reason:
          'GitHub reach revocation is still converging. Writes stay disabled until CMA observes a tombstone acknowledgement, confirmed agent-key revocation, or bounded expiry.',
        action: 'refresh_reach',
        label: 'Check reach status',
      };
    case 'revoked':
      return {
        reason:
          'GitHub reach for this Launch was revoked. Reauthorize or select another eligible GitHub connection before continuing.',
        action: 'recover_connection',
        label: 'Review GitHub access',
      };
    case 'expired':
      return {
        reason:
          'GitHub reach for this Launch expired. Reauthorize or select another eligible GitHub connection before continuing.',
        action: 'recover_connection',
        label: 'Review GitHub access',
      };
    case undefined:
      return {
        reason:
          'CMA has no GitHub reach status for this Launch. Select an eligible GitHub connection before continuing.',
        action: 'recover_connection',
        label: 'Review GitHub access',
      };
  }
}

function validationRemediation(artifact: DiffArtifact): ContributionRemediation | null {
  const recapture = (reason: string, label = 'Recapture diff'): ContributionRemediation => ({
    reason,
    action: 'capture',
    label,
  });
  const fixInSession = (reason: string): ContributionRemediation => ({
    reason,
    action: 'session',
    label: 'Open session',
  });

  if (
    artifact.validation === 'stale_head' ||
    artifact.expected_branch_head !== artifact.observed_branch_head
  ) {
    return recapture(
      'The Launch branch head changed after this diff was sealed. Recapture and review the current head; CMA will not force-push it.',
    );
  }

  const outcome: DiffValidationOutcome = artifact.validation;
  switch (outcome) {
    case 'valid': {
      if (artifact.total_changed_files === 0) {
        return fixInSession(
          'No workspace changes were found. Continue editing in the session, then refresh the diff.',
        );
      }
      if (artifact.total_changed_files > artifact.changed_file_limit) {
        return fixInSession(
          `This diff changes ${String(artifact.total_changed_files)} files, above the ${String(artifact.changed_file_limit)}-file limit. Reduce the change set, then refresh the diff.`,
        );
      }
      const failed = artifact.checks.find((check) => !check.passed);
      return failed === undefined
        ? null
        : fixInSession(
            `Required check ${failed.key} failed: ${failed.summary} Fix the reported workspace state, then refresh the diff.`,
          );
    }
    case 'no_changes':
      return fixInSession(
        'No workspace changes were found. Continue editing in the session, then refresh the diff.',
      );
    case 'workflow_scope_required':
      return fixInSession(
        'Workflow-file publication is not authorized for this Launch. Remove those workflow changes in the session, then refresh the diff.',
      );
    case 'too_many_changes':
      return fixInSession(
        `This diff changes ${String(artifact.total_changed_files)} files, above the ${String(artifact.changed_file_limit)}-file limit. Reduce the change set, then refresh the diff.`,
      );
    case 'too_many_bytes':
      return fixInSession(
        "This diff exceeds CMA's changed-byte limit. Reduce the changed content in the session, then refresh the diff.",
      );
    case 'unsupported_file':
      return fixInSession(
        'This diff contains an unsupported file type. Remove or replace the unsupported file in the session, then refresh the diff.',
      );
    case 'artifact_too_large':
      return fixInSession(
        "The complete review artifact exceeds CMA's safe size limit. Reduce the change set in the session, then refresh the diff.",
      );
    case 'missing_baseline':
      return recapture(
        'CMA could not read the sealed workspace baseline. Retry diff capture; no GitHub write is authorized from this artifact.',
        'Retry diff capture',
      );
    case 'corrupt_checkpoint':
      return recapture(
        'The captured workspace checkpoint could not be verified. Capture a new checkpoint; no GitHub write is authorized from this artifact.',
        'Recapture diff',
      );
  }
}

function mode(value: number | null): string {
  return value === null ? 'none' : `0${value.toString(8)}`;
}

function patchStats(file: DiffFile): { readonly additions: number; readonly deletions: number } {
  if (file.patch === undefined) return { additions: 0, deletions: 0 };
  let additions = 0;
  let deletions = 0;
  for (const line of file.patch.split('\n')) {
    if (line.startsWith('+') && !line.startsWith('+++')) additions += 1;
    if (line.startsWith('-') && !line.startsWith('---')) deletions += 1;
  }
  return { additions, deletions };
}

function checksLabel(value: string): string {
  return value === 'awaiting_maintainer_approval'
    ? 'Awaiting maintainer approval'
    : stateLabel(value);
}

interface PullRequestStatusGuidance {
  readonly reason: string;
  readonly refreshable: boolean;
}

function pullRequestStatusGuidance(state: PullRequestState): PullRequestStatusGuidance | null {
  switch (state) {
    case 'creating':
      return {
        reason:
          'Pull request creation is still reconciling. CMA will not open a second pull request while this provider operation is unresolved.',
        refreshable: true,
      };
    case 'updating':
      return {
        reason:
          'Pull request update is still reconciling. CMA will not issue a second update while this provider operation is unresolved.',
        refreshable: true,
      };
    case 'recovery_required':
      return {
        reason:
          'The pull request outcome is ambiguous and requires provider reconciliation. CMA will not issue another pull request write while recovery is required.',
        refreshable: true,
      };
    case 'closed_unmerged':
      return {
        reason: 'This pull request is closed. Create a New Launch for a separate attempt.',
        refreshable: false,
      };
    case 'merged':
      return {
        reason: 'A merged pull request cannot be updated from CMA.',
        refreshable: false,
      };
    case 'open':
      return null;
  }
}

function downloadableName(launchId: string, artifactId: string): string {
  return `cma-${launchId}-${artifactId}.json`;
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

function pendingPullRequestOperation(
  pending: PendingBrowserCommand | null,
  launchId: string,
): 'open' | 'update' | null {
  if (pending?.resource !== `launch:${launchId}` || pending.action !== 'confirm-pull-request') {
    return null;
  }
  if (pending.slot === `launch:pull-request:${launchId}:open`) return 'open';
  if (pending.slot === `launch:pull-request:${launchId}:update`) return 'update';
  return null;
}

function pullRequestCommandMetadata(
  pending: PendingBrowserCommand | null,
  launchId: string,
  operation: 'open' | 'update',
): PullRequestCommandMetadata | undefined {
  return retainedCommandMetadata<PullRequestCommandMetadata>(
    pending,
    {
      slot: `launch:pull-request:${launchId}:${operation}`,
      resource: `launch:${launchId}`,
      action: 'confirm-pull-request',
    },
    (value) =>
      commandObject<PullRequestCommandMetadata>(value, {
        launchId: exactString(launchId),
        operation: oneOfLiterals([operation]),
        expectedRevision: nonNegativeInteger,
        snapshotDigest: nonEmptyString,
      }),
  );
}

export function ContributionSurface(props: ContributionSurfaceProps) {
  return <ContributionSurfaceRoute key={props.launchId} {...props} />;
}

function ContributionSurfaceRoute({ ownerId, launchId, navigate }: ContributionSurfaceProps) {
  const resource = useLaunchResource(launchId);
  const [review, setReview] = useState<DiffReview | null>(null);
  const [reviewFailure, setReviewFailure] = useState<string | null>(null);
  const [busy, setBusy] = useState<Busy>(null);
  const [confirmingPublish, setConfirmingPublish] = useState(false);
  const [proposal, setProposal] = useState<PullRequestDraft | null>(null);
  const [proposalTitle, setProposalTitle] = useState('');
  const [proposalBody, setProposalBody] = useState('');
  const heading = useRef<HTMLHeadingElement>(null);
  const commands = useIdempotentCommand(ownerId, [`launch:${launchId}`]);

  useEffect(() => {
    heading.current?.focus();
  }, [launchId, resource.loading]);

  useEffect(() => {
    const controller = new AbortController();
    void readLaunchDiff(launchId, controller.signal)
      .then((next) => {
        if (controller.signal.aborted) return;
        setReview((current) =>
          current !== null && current.revision_number > next.revision_number ? current : next,
        );
        setReviewFailure(null);
      })
      .catch((cause: unknown) => {
        if (!controller.signal.aborted) {
          setReviewFailure(cause instanceof Error ? cause.message : String(cause));
        }
      });
    return () => {
      controller.abort();
    };
  }, [launchId, resource.launch?.revision_number]);

  async function capture(): Promise<void> {
    const launch = resource.launch;
    if (launch === null) return;
    setBusy('capture');
    setReviewFailure(null);
    const slot = `launch:diff:${launch.id}`;
    try {
      const identity = {
        slot,
        resource: `launch:${launch.id}`,
        action: 'capture-diff',
      } as const;
      const request = retainedCommandRequest(
        commands.pending,
        identity,
        {
          launchId: launch.id,
          expectedRevision: launch.revision_number,
          snapshotDigest: launch.effective.digest,
        },
        (value) => launchRevisionDigestRequest(value, launch.id),
      );
      const next = await commands.execute(
        { ...identity, request, persistRequest: true },
        'diff capture',
        (idempotencyKey, frozen) =>
          requestLaunchDiff(frozen.launchId, frozen.expectedRevision, frozen.snapshotDigest, {
            idempotencyKey,
          }),
      );
      resource.accept(next);
    } catch (cause) {
      setReviewFailure(
        commandOutcomeIsUnresolved(cause)
          ? 'The diff-capture outcome is unknown. CMA retained the exact command for reconciliation.'
          : cause instanceof Error
            ? cause.message
            : String(cause),
      );
    } finally {
      setBusy(null);
    }
  }

  async function publish(): Promise<void> {
    const launch = resource.launch;
    const artifact = review?.artifact;
    if (
      launch === null ||
      ((artifact === null || artifact === undefined) &&
        commands.pending?.slot !== `launch:branch:${launch?.id}`)
    ) {
      return;
    }
    setBusy('publish');
    setReviewFailure(null);
    const slot = `launch:branch:${launch.id}`;
    try {
      const identity = {
        slot,
        resource: `launch:${launch.id}`,
        action: 'publish-branch',
      } as const;
      const request = retainedCommandRequest(
        commands.pending,
        identity,
        {
          launchId: launch.id,
          expectedRevision: launch.revision_number,
          snapshotDigest: artifact?.digest ?? '',
        },
        (value) => launchRevisionDigestRequest(value, launch.id),
      );
      if (request.snapshotDigest === '') return;
      const next = await commands.execute(
        { ...identity, request, persistRequest: true },
        'branch publication',
        (idempotencyKey, frozen) =>
          publishLaunchBranch(frozen.launchId, frozen.expectedRevision, frozen.snapshotDigest, {
            idempotencyKey,
          }),
      );
      resource.accept(next);
      setConfirmingPublish(false);
    } catch (cause) {
      if (!commandOutcomeIsUnresolved(cause)) setConfirmingPublish(false);
      setReviewFailure(
        commandOutcomeIsUnresolved(cause)
          ? 'The branch-publication outcome is unknown. CMA retained the exact reviewed artifact and command for reconciliation.'
          : cause instanceof Error
            ? cause.message
            : String(cause),
      );
      await resource.refresh();
    } finally {
      setBusy(null);
    }
  }

  async function download(): Promise<void> {
    const artifact = review?.artifact;
    if (artifact === null || artifact === undefined) return;
    setBusy('download');
    setReviewFailure(null);
    try {
      const bytes = await downloadDiffArtifact(launchId, artifact.id);
      const body = new ArrayBuffer(bytes.byteLength);
      new Uint8Array(body).set(bytes);
      const url = URL.createObjectURL(new Blob([body], { type: 'application/octet-stream' }));
      const anchor = document.createElement('a');
      anchor.href = url;
      anchor.download = downloadableName(launchId, artifact.id);
      anchor.click();
      URL.revokeObjectURL(url);
    } catch (cause) {
      setReviewFailure(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(null);
    }
  }

  async function prepareProposal(operation: 'open' | 'update'): Promise<void> {
    setBusy('proposal');
    setReviewFailure(null);
    try {
      const draft = await reviewPullRequest(launchId, operation);
      setProposal(draft);
      setProposalTitle(draft.title);
      setProposalBody(draft.body);
    } catch (cause) {
      setReviewFailure(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(null);
    }
  }

  async function confirmProposal(): Promise<void> {
    const launch = resource.launch;
    const current = proposal;
    const retainedOperation = pendingPullRequestOperation(commands.pending, launchId);
    if (launch === null || (current === null && retainedOperation === null)) return;
    setBusy('pr');
    setReviewFailure(null);
    const operation = retainedOperation ?? current?.operation;
    if (operation === undefined) return;
    const slot = `launch:pull-request:${launch.id}:${operation}`;
    try {
      const identity = {
        slot,
        resource: `launch:${launch.id}`,
        action: 'confirm-pull-request',
      } as const;
      let request: PullRequestCommandRequest;
      const retained = pullRequestCommandMetadata(commands.pending, launchId, operation);
      if (retained !== undefined) {
        request = {
          ...retained,
          edits: { title: proposalTitle, body: proposalBody },
        };
      } else {
        if (current === null) return;
        const edits = { title: proposalTitle, body: proposalBody };
        const sealed = await reviewPullRequest(launch.id, current.operation, edits);
        request = {
          launchId: launch.id,
          operation: sealed.operation,
          expectedRevision: launch.revision_number,
          snapshotDigest: sealed.intent_digest,
          edits,
        };
      }
      const next = await commands.execute(
        {
          ...identity,
          request,
          retained: {
            launchId: request.launchId,
            operation: request.operation,
            expectedRevision: request.expectedRevision,
            snapshotDigest: request.snapshotDigest,
          } satisfies PullRequestCommandMetadata,
        },
        'pull-request confirmation',
        (idempotencyKey, frozen) =>
          confirmPullRequest(
            frozen.launchId,
            frozen.operation,
            frozen.expectedRevision,
            frozen.snapshotDigest,
            frozen.edits,
            { idempotencyKey },
          ),
      );
      resource.accept(next);
      setProposal(null);
    } catch (cause) {
      setReviewFailure(
        commandOutcomeIsUnresolved(cause)
          ? 'The pull-request outcome is unknown. CMA retained the command fingerprint and reviewed head, but not the title or body.'
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
            Contribution review
          </h1>
        </header>
        <div className="trigger-workflow__loading">
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
              Contribution unavailable
            </h1>
            <p>{resource.failure ?? 'This Launch could not be read.'}</p>
          </div>
        </section>
      </main>
    );
  }

  const launch = resource.launch;
  const captureSlot = `launch:diff:${launch.id}`;
  const publishSlot = `launch:branch:${launch.id}`;
  const retainedProposalOperation = pendingPullRequestOperation(commands.pending, launch.id);
  const proposalOperation = proposal?.operation ?? retainedProposalOperation;
  const proposalSlot =
    proposalOperation === undefined
      ? null
      : `launch:pull-request:${launch.id}:${proposalOperation}`;
  const pendingCapture = commands.pending?.slot === captureSlot;
  const pendingPublish = commands.pending?.slot === publishSlot;
  const pendingProposal =
    proposalSlot !== null &&
    commands.pending?.slot === proposalSlot &&
    commands.pending.action === 'confirm-pull-request';
  const repository = upstreamRepository(launch);
  const artifact = review?.artifact ?? launch.diff;
  const contribution = review?.contribution ?? launch.contribution;
  const pullRequest = review?.pull_request ?? launch.pull_request;
  const reach = reachState(launch);
  const reachAvailable = reach === 'active';
  const githubRemediation = reachRemediation(reach);
  const reviewRemediation = artifact === null ? null : validationRemediation(artifact);
  const publishRemediation = githubRemediation ?? reviewRemediation;
  const exactReviewPublished = artifact !== null && contribution?.diff_artifact_id === artifact.id;
  const canPublish =
    artifact !== null && reviewRemediation === null && reachAvailable && !exactReviewPublished;
  const operation = pullRequest === null ? 'open' : pullRequest.state === 'open' ? 'update' : null;
  const pullRequestGuidance =
    pullRequest === null ? null : pullRequestStatusGuidance(pullRequest.state);
  const launchRoute = { kind: 'launch', launchId } as const;
  const recoveryRoute = { kind: 'trigger_recovery', launchId } as const;
  const linkedSessionId = sessionId(launch);
  const linkedSessionRoute =
    linkedSessionId === undefined
      ? null
      : ({ kind: 'session', sessionId: linkedSessionId } as const);
  const canPrepareProposal =
    operation !== null && reachAvailable && reviewRemediation === null && exactReviewPublished;
  const publishReason =
    publishRemediation?.reason ??
    (exactReviewPublished
      ? 'This exact reviewed artifact is already published.'
      : 'Publishing writes exactly this reviewed artifact to the guarded Launch branch.');
  const pullRequestReason =
    pullRequestGuidance?.reason ??
    publishRemediation?.reason ??
    (artifact === null
      ? 'Capture and review a workspace diff before preparing a pull request.'
      : !exactReviewPublished
        ? 'Publish this exact reviewed artifact before preparing a pull request.'
        : `${operation === 'open' ? 'Opening' : 'Updating'} remains a separate confirmation.`);

  function remediationAction(remediation: ContributionRemediation | null) {
    if (remediation === null) return null;
    if (remediation.action === 'capture') {
      return (
        <Button
          size="sm"
          variant="outline"
          icon={<IconRefreshOutline16 />}
          disabled={
            busy !== null ||
            launch.session === null ||
            (commands.pending !== null && !pendingCapture)
          }
          onClick={() => void capture()}
        >
          {pendingCapture ? 'Reconcile diff capture' : remediation.label}
        </Button>
      );
    }
    if (remediation.action === 'refresh_reach') {
      return (
        <Button
          size="sm"
          variant="outline"
          icon={<IconRefreshOutline16 />}
          disabled={resource.loading || busy !== null}
          onClick={() => void resource.refresh()}
        >
          {remediation.label}
        </Button>
      );
    }

    const route = remediation.action === 'recover_connection' ? recoveryRoute : linkedSessionRoute;
    if (route === null) return null;
    return (
      <a
        className="button button--outline button--sm"
        href={routePath(route)}
        onClick={(event) => routeClick(event, route, navigate)}
      >
        <span className="button__icon" aria-hidden="true">
          {remediation.action === 'recover_connection' ? (
            <IconLinkOutline16 />
          ) : (
            <IconRightUpOutline16 />
          )}
        </span>
        {remediation.label}
      </a>
    );
  }

  return (
    <main className="trigger-workflow contribution">
      <header className="trigger-workflow__head">
        <div>
          <p className="eyebrow">Contribution review</p>
          <h1 ref={heading} tabIndex={-1}>
            {repository.canonical_owner}/{repository.canonical_name}
          </h1>
          <p className="trigger-workflow__repo">
            <IconBranchOutline16 />
            {launch.branch?.name ?? 'Launch branch pending'}
          </p>
        </div>
        <a
          href={routePath(launchRoute)}
          onClick={(event) => routeClick(event, launchRoute, navigate)}
        >
          Launch progress
        </a>
      </header>

      {resource.offline ? (
        <div className="launch__offline" role="status">
          Offline · review data retained · Last updated{' '}
          {resource.lastUpdated === null
            ? 'unknown'
            : new Date(resource.lastUpdated).toLocaleTimeString()}
        </div>
      ) : null}
      {!resource.offline && resource.failure !== null ? (
        <div className="notice notice--fail trigger-workflow__notice" role="alert">
          <IconWarningOutline16 />
          <div>
            <p className="notice__title">Contribution data may be stale.</p>
            <p className="notice__detail">{resource.failure}</p>
            <Button
              size="sm"
              variant="outline"
              icon={<IconRefreshOutline16 />}
              onClick={() => void resource.refresh()}
            >
              Retry refresh
            </Button>
          </div>
        </div>
      ) : null}
      {commands.pending === null ? null : (
        <div className="notice notice--quiet trigger-workflow__notice" role="status">
          <IconRefreshOutline16 />
          <div>
            <p className="notice__title">Reconciling {commands.pending.label}</p>
            <p className="notice__detail">
              Only the retained same-key command can be retried until CMA returns a finite result.
            </p>
          </div>
        </div>
      )}
      {reviewFailure === null ? null : (
        <div className="notice notice--fail trigger-workflow__notice" role="alert">
          <IconWarningOutline16 />
          <div>
            <p className="notice__title">The contribution action did not complete.</p>
            <p className="notice__detail">{reviewFailure}</p>
          </div>
        </div>
      )}

      <div className="trigger-workflow__scroll">
        <section className="launch-band" aria-labelledby="contribution-snapshot-heading">
          <div className="launch-band__heading">
            <IconRefreshOutline16 />
            <h2 id="contribution-snapshot-heading">Workspace snapshot</h2>
          </div>
          <dl className="launch-facts">
            <div>
              <dt>Source</dt>
              <dd>
                <code>{shortRevision(launch.source_sha)}</code>
              </dd>
            </div>
            <div>
              <dt>Branch head</dt>
              <dd>
                {launch.branch === null ? (
                  'Pending'
                ) : (
                  <code>{shortRevision(launch.branch.current_head)}</code>
                )}
              </dd>
            </div>
            <div>
              <dt>Reviewed</dt>
              <dd>{artifact === null ? 'Not yet' : timestamp(artifact.created_at)}</dd>
            </div>
          </dl>
          <div className="contribution__row">
            <span className="control-reason">
              Capturing a diff reads the workspace and writes nothing to GitHub.
            </span>
            <Button
              variant="outline"
              disabled={
                busy !== null ||
                launch.session === null ||
                (commands.pending !== null && !pendingCapture)
              }
              onClick={() => void capture()}
            >
              {busy === 'capture'
                ? pendingCapture
                  ? 'Reconciling'
                  : 'Submitting'
                : pendingCapture
                  ? 'Reconcile diff capture'
                  : artifact === null
                    ? 'Review diff'
                    : 'Refresh diff'}
            </Button>
          </div>
        </section>

        {artifact === null ? (
          <section className="launch-band contribution__empty" aria-label="No reviewed diff">
            <p>No sealed workspace diff is available yet.</p>
          </section>
        ) : (
          <>
            {artifact.workflow_paths.length === 0 ? null : (
              <section
                className="trigger-workflowwarning"
                aria-labelledby="contribution-workflows-heading"
              >
                <IconWarningOutline16 />
                <div>
                  <h2 id="contribution-workflows-heading">Workflow files changed</h2>
                  <ul>
                    {artifact.workflow_paths.map((path) => (
                      <li key={path}>
                        <code>{path}</code>
                      </li>
                    ))}
                  </ul>
                  <p>
                    GitHub may hold these checks for maintainer approval. This is not a CMA retry
                    failure.
                  </p>
                </div>
              </section>
            )}

            <section className="launch-band" aria-labelledby="contribution-diff-heading">
              <div className="launch-band__heading">
                <IconBranchOutline16 />
                <h2 id="contribution-diff-heading">Changed files</h2>
              </div>
              <div className="contribution__summary">
                <strong>{String(artifact.total_changed_files)} files</strong>
                <span>{artifact.changed_bytes.toLocaleString('en')} changed bytes</span>
                <span>{stateLabel(artifact.validation)}</span>
              </div>
              <div className="contribution-files">
                {artifact.files.map((file) => {
                  const stats = patchStats(file);
                  return (
                    <details key={file.path} className="contribution-file">
                      <summary>
                        <IconChevronRightOutline14 />
                        <span className="contribution-file__path">{file.path}</span>
                        <span className="contribution-file__change">{stateLabel(file.change)}</span>
                        <span className="contribution-file__stats">
                          +{String(stats.additions)} −{String(stats.deletions)}
                        </span>
                      </summary>
                      <dl>
                        <div>
                          <dt>Content</dt>
                          <dd>{file.binary ? 'Binary' : 'Text'}</dd>
                        </div>
                        <div>
                          <dt>Mode</dt>
                          <dd>
                            {mode(file.old_mode)} → {mode(file.new_mode)}
                          </dd>
                        </div>
                        <div>
                          <dt>Size</dt>
                          <dd>
                            {file.old_bytes.toLocaleString('en')} →{' '}
                            {file.new_bytes.toLocaleString('en')} bytes
                          </dd>
                        </div>
                        <div>
                          <dt>Patch</dt>
                          <dd>
                            {file.patch_complete
                              ? 'Complete in this review'
                              : 'Use the complete artifact download'}
                          </dd>
                        </div>
                      </dl>
                      {file.binary || file.patch === undefined ? null : <pre>{file.patch}</pre>}
                    </details>
                  );
                })}
              </div>
              <div className="contribution__row">
                <span className="control-reason">
                  Complete artifact retained until {timestamp(artifact.delete_after)}
                </span>
                <Button
                  icon={<IconDownloadOutline16 />}
                  disabled={busy !== null}
                  onClick={() => void download()}
                >
                  {busy === 'download' ? 'Downloading' : 'Download artifact'}
                </Button>
              </div>
            </section>

            <section className="launch-band" aria-labelledby="contribution-attribution-heading">
              <div className="launch-band__heading">
                <IconCheckOutline16 />
                <h2 id="contribution-attribution-heading">Commit attribution</h2>
              </div>
              <dl className="launch-facts">
                <div>
                  <dt>Author</dt>
                  <dd>
                    {artifact.author.name}
                    <br />
                    <code>{artifact.author.email}</code>
                  </dd>
                </div>
                <div>
                  <dt>Committer</dt>
                  <dd>
                    {artifact.committer.name}
                    <br />
                    <code>{artifact.committer.email}</code>
                  </dd>
                </div>
                <div>
                  <dt>Expected head</dt>
                  <dd>
                    <code>{shortRevision(artifact.expected_branch_head)}</code>
                  </dd>
                </div>
              </dl>
              <div className="contribution-checks">
                {artifact.checks.map((check) => (
                  <p key={check.key} data-passed={check.passed ? '' : undefined}>
                    <IconCheckOutline16 />
                    <span>
                      <strong>{check.key}</strong>
                      {check.summary}
                    </span>
                  </p>
                ))}
              </div>
              <div className="contribution__row">
                <span id="contribution-publish-reason" className="control-reason">
                  {publishReason}
                </span>
                <div className="contribution__actions">
                  {remediationAction(publishRemediation)}
                  <Button
                    variant="primary"
                    aria-describedby="contribution-publish-reason"
                    disabled={!canPublish || busy !== null || commands.pending !== null}
                    onClick={() => setConfirmingPublish(true)}
                  >
                    Publish branch
                  </Button>
                </div>
              </div>
            </section>
          </>
        )}

        <section className="launch-band" aria-labelledby="contribution-pr-heading">
          <div className="launch-band__heading">
            <IconRightUpOutline16 />
            <h2 id="contribution-pr-heading">Pull request</h2>
          </div>
          {pullRequest === null ? (
            <p className="launch-band__empty">No pull request has been opened.</p>
          ) : (
            <dl className="launch-facts">
              <div>
                <dt>Request</dt>
                <dd>
                  {pullRequest.canonical_url === null ? (
                    stateLabel(pullRequest.state)
                  ) : (
                    <a href={pullRequest.canonical_url} target="_blank" rel="noopener noreferrer">
                      #{String(pullRequest.number)} {pullRequest.title}
                      <IconRightUpOutline16 />
                    </a>
                  )}
                </dd>
              </div>
              <div>
                <dt>Head</dt>
                <dd>
                  <code>
                    {shortRevision(pullRequest.head_revision ?? pullRequest.expected_head_revision)}
                  </code>
                </dd>
              </div>
              <div>
                <dt>Base</dt>
                <dd>
                  <code>{pullRequest.base_ref}</code>
                </dd>
              </div>
              <div>
                <dt>Mergeability</dt>
                <dd>{mergeabilityLabel(pullRequest.mergeability)}</dd>
              </div>
              <div>
                <dt>Checks</dt>
                <dd>{checksLabel(pullRequest.checks)}</dd>
              </div>
              <div>
                <dt>State</dt>
                <dd>{stateLabel(pullRequest.state)}</dd>
              </div>
            </dl>
          )}
          <div className="contribution__row">
            <span id="contribution-pr-reason" className="control-reason">
              {pullRequestReason}
            </span>
            <div className="contribution__actions">
              {remediationAction(publishRemediation)}
              {pullRequestGuidance?.refreshable === true ? (
                <Button
                  variant="outline"
                  icon={<IconRefreshOutline16 />}
                  disabled={resource.loading || busy !== null}
                  onClick={() => void resource.refresh()}
                >
                  Refresh status
                </Button>
              ) : operation === null ? null : (
                <Button
                  variant="primary"
                  aria-describedby="contribution-pr-reason"
                  disabled={!canPrepareProposal || busy !== null || commands.pending !== null}
                  onClick={() => void prepareProposal(operation)}
                >
                  {busy === 'proposal'
                    ? 'Reviewing'
                    : operation === 'open'
                      ? 'Open pull request'
                      : 'Update pull request'}
                </Button>
              )}
            </div>
          </div>
        </section>
      </div>

      <Modal
        open={confirmingPublish || pendingPublish}
        title="Publish this reviewed branch?"
        description="CMA will create one commit with the author and committer shown in this review, then advance only the guarded Launch branch if its head is unchanged."
        onClose={() => {
          if (busy === null && !pendingPublish) setConfirmingPublish(false);
        }}
        footer={
          <>
            <Button
              disabled={busy !== null || pendingPublish}
              onClick={() => setConfirmingPublish(false)}
            >
              Cancel
            </Button>
            <Button
              variant="primary"
              disabled={
                !reachAvailable || busy !== null || (commands.pending !== null && !pendingPublish)
              }
              onClick={() => void publish()}
            >
              {busy === 'publish'
                ? pendingPublish
                  ? 'Reconciling'
                  : 'Submitting'
                : pendingPublish
                  ? 'Reconcile branch publication'
                  : 'Publish branch'}
            </Button>
          </>
        }
      />

      <Modal
        open={proposal !== null || pendingProposal}
        title={`${proposalOperation === 'update' ? 'Update' : 'Open'} pull request?`}
        description="This writes the exact reviewed head to the upstream repository's pull-request list. Review and merge remain on GitHub."
        onClose={() => {
          if (busy === null && !pendingProposal) setProposal(null);
        }}
        footer={
          <>
            <Button disabled={busy !== null || pendingProposal} onClick={() => setProposal(null)}>
              Cancel
            </Button>
            <Button
              variant="primary"
              disabled={
                !reachAvailable ||
                busy !== null ||
                proposalTitle.trim() === '' ||
                (commands.pending !== null && !pendingProposal)
              }
              onClick={() => void confirmProposal()}
            >
              {busy === 'pr'
                ? pendingProposal
                  ? 'Reconciling'
                  : 'Submitting'
                : pendingProposal
                  ? 'Reconcile pull request'
                  : proposalOperation === 'update'
                    ? 'Update pull request'
                    : 'Open pull request'}
            </Button>
          </>
        }
      >
        <div className="contribution-proposal">
          {proposalOperation === 'update' ? (
            <p className="notice notice--quiet">
              Provider title and body are preserved unless you explicitly edit them. CMA refuses a
              concurrent provider change.
            </p>
          ) : null}
          <label className="field">
            <span className="field__label">Title</span>
            <Input
              placement="field__input"
              value={proposalTitle}
              disabled={pendingProposal && !commands.pending?.requiresRequestReconstruction}
              onChange={(event) => setProposalTitle(event.target.value)}
            />
          </label>
          <label className="field">
            <span className="field__label">Body</span>
            <textarea
              className="field__input"
              value={proposalBody}
              disabled={pendingProposal && !commands.pending?.requiresRequestReconstruction}
              onChange={(event) => setProposalBody(event.target.value)}
            />
          </label>
          <dl className="launch-facts">
            <div>
              <dt>Base</dt>
              <dd>{proposal?.base_ref}</dd>
            </div>
            <div>
              <dt>Head</dt>
              <dd>{proposal?.head_ref}</dd>
            </div>
            <div>
              <dt>Revision</dt>
              <dd>
                <code>
                  {proposal === null ? '' : shortRevision(proposal.expected_head_revision)}
                </code>
              </dd>
            </div>
          </dl>
        </div>
      </Modal>
    </main>
  );
}
