import { useEffect, useRef, useState } from 'react';
import type {
  DiffArtifact,
  DiffFile,
  DiffReview,
  DiffValidationOutcome,
  LaunchCapabilityRevocationState,
  LaunchFailure,
  PublicationState,
  PullRequestDraft,
  PullRequestRecord,
  PullRequestState,
} from '@chronoai/cma-sdk';
import {
  completeLaunch,
  confirmPullRequest,
  createNewLaunch,
  downloadDiffArtifact,
  publishLaunchBranch,
  readLaunchDiff,
  refreshPullRequestStatus,
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
import { useRetryDeadline } from './useRetryDeadline';
import {
  commandOutcomeIsUnresolved,
  retainedCommandMetadata,
  retainedCommandRequest,
  useIdempotentCommand,
} from './useIdempotentCommand';
import type { PendingBrowserCommand } from './useIdempotentCommand';
import {
  commandBoolean,
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

type Busy =
  'capture' | 'publish' | 'proposal' | 'pr' | 'download' | 'status' | 'complete' | 'new' | null;

interface PullRequestCommandMetadata {
  readonly launchId: string;
  readonly operation: 'open' | 'update';
  readonly expectedRevision: number;
  readonly snapshotDigest: string;
  readonly updateMetadata: boolean;
}

interface PullRequestCommandRequest extends PullRequestCommandMetadata {
  readonly edits: { readonly title?: string; readonly body?: string };
}

interface LaunchRevisionDigestRequest {
  readonly launchId: string;
  readonly expectedRevision: number;
  readonly snapshotDigest: string;
}

interface PullRequestStatusRefreshRequest {
  readonly launchId: string;
  readonly expectedRevision: number;
}

interface BranchPublicationReview extends LaunchRevisionDigestRequest {
  readonly artifactId: string;
  readonly expectedBranchHead: string;
}

interface PullRequestReview {
  readonly draft: PullRequestDraft;
  readonly expectedRevision: number;
}

const PULL_REQUEST_DRAFT_FIELDS: Record<keyof PullRequestDraft, true> = {
  launch_id: true,
  operation: true,
  marker: true,
  upstream_repository_id: true,
  upstream_repository_node_id: true,
  fork_repository_id: true,
  fork_repository_node_id: true,
  base_ref: true,
  head_owner: true,
  head_branch: true,
  head_ref: true,
  expected_base_revision: true,
  expected_head_revision: true,
  previous_head_revision: true,
  target_number: true,
  target_provider_id: true,
  target_provider_node_id: true,
  update_metadata: true,
  expected_previous_title: true,
  expected_previous_body_digest: true,
  title: true,
  body: true,
  body_digest: true,
  intent_digest: true,
};

const PULL_REQUEST_REVIEW_DRIFT_MESSAGE =
  'The pull-request proposal changed while CMA resealed it. Review the updated target, revisions, content, and intent before confirming again; CMA did not write to GitHub.';
const PULL_REQUEST_STATUS_POLL_DELAYS_MS = [0, 250, 750, 1_500, 3_000] as const;

function freezePullRequestDraft(draft: PullRequestDraft): PullRequestDraft {
  return Object.fromEntries(
    (Object.keys(PULL_REQUEST_DRAFT_FIELDS) as (keyof PullRequestDraft)[]).map((field) => [
      field,
      draft[field],
    ]),
  ) as unknown as PullRequestDraft;
}

function samePullRequestDraft(left: PullRequestDraft, right: PullRequestDraft): boolean {
  return (Object.keys(PULL_REQUEST_DRAFT_FIELDS) as (keyof PullRequestDraft)[]).every(
    (field) => left[field] === right[field],
  );
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

function pullRequestStatusFingerprint(record: PullRequestRecord): string {
  return JSON.stringify([
    record.state,
    record.head_revision,
    record.mergeability,
    record.checks,
    record.updated_at,
    record.provider_updated_at,
    record.provider_request_id,
    record.head_matches_launch,
    record.status_failure?.code ?? null,
    record.status_failure?.observed_at ?? null,
  ]);
}

function pullRequestStatusFailureReason(failure: LaunchFailure): string {
  const retry =
    failure.retry_at === undefined ? '' : ` CMA will retry after ${timestamp(failure.retry_at)}.`;
  return `${failure.detail} ${failure.remediation}${retry}`;
}

function publicationStatusReason(state: PublicationState): string {
  switch (state) {
    case 'intent_recorded':
      return 'Publication intent is recorded, but Git object writes are not yet confirmed. CMA is reconciling this exact intent; pull-request preparation stays disabled.';
    case 'objects_written':
      return 'Git objects are written, but the guarded Launch branch has not been advanced. CMA is reconciling the exact ref update; pull-request preparation stays disabled.';
    case 'ref_update_pending':
      return 'The guarded branch update may have reached GitHub and is awaiting read-only reconciliation. CMA will not repeat the ref write or prepare a pull request yet.';
    case 'published':
      return 'This exact reviewed artifact is published on the guarded Launch branch.';
    case 'conflict':
      return 'The guarded Launch branch changed before publication completed. CMA will not force-push it; review Launch recovery or create a New Launch for a separate attempt.';
    case 'recovery_required':
      return 'The publication outcome is ambiguous and CMA has paused automatic writes. Review the Launch recovery evidence before taking another action.';
    case 'failed':
      return 'Branch publication failed and CMA does not claim a published result. Review the Launch failure and remediation before taking another action.';
  }
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
      return {
        reason:
          'Refresh queues an immediate read-only GitHub status check. Updating the pull request remains a separate confirmation.',
        refreshable: true,
      };
  }
}

type SameLaunchContributionBlock = 'merged' | 'closed_unmerged' | 'head_drift';

function sameLaunchContributionBlock(
  launchPullRequest: PullRequestRecord | null,
  reviewPullRequest: PullRequestRecord | null | undefined,
): SameLaunchContributionBlock | null {
  const records = [launchPullRequest, reviewPullRequest ?? null];
  if (records.some((record) => record?.state === 'merged')) return 'merged';
  if (records.some((record) => record?.state === 'closed_unmerged')) return 'closed_unmerged';
  return records.some((record) => record?.head_matches_launch === false) ? 'head_drift' : null;
}

function sameLaunchContributionBlockReason(
  block: SameLaunchContributionBlock | null,
): string | null {
  switch (block) {
    case 'merged':
      return 'This pull request was merged. The same Launch cannot capture another diff, publish another branch state, or update the completed contribution. Create a New Launch for a separate attempt.';
    case 'closed_unmerged':
      return 'This pull request is closed. The same Launch cannot capture another diff, publish another branch state, or reopen the contribution. Create a New Launch for a separate attempt.';
    case 'head_drift':
      return 'The pull-request head changed outside CMA. Status and history remain readable, but the same Launch cannot capture another diff, publish another branch state, or update the pull request. Create a New Launch for a fresh reviewed attempt.';
    case null:
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

function pullRequestStatusRefreshRequest(
  value: unknown,
  launchId: string,
): PullRequestStatusRefreshRequest | null {
  return commandObject<PullRequestStatusRefreshRequest>(value, {
    launchId: exactString(launchId),
    expectedRevision: nonNegativeInteger,
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
        updateMetadata: commandBoolean,
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
  const [publicationReview, setPublicationReview] = useState<BranchPublicationReview | null>(null);
  const [proposalReview, setProposalReview] = useState<PullRequestReview | null>(null);
  const [proposalReviewNotice, setProposalReviewNotice] = useState<string | null>(null);
  const [proposalTitle, setProposalTitle] = useState('');
  const [proposalBody, setProposalBody] = useState('');
  const [proposalMetadataEdited, setProposalMetadataEdited] = useState(false);
  const [confirmingComplete, setConfirmingComplete] = useState(false);
  const [confirmingNew, setConfirmingNew] = useState(false);
  const [statusNotice, setStatusNotice] = useState<string | null>(null);
  const proposal = proposalReview?.draft ?? null;
  const proposalText = useRef({ title: proposalTitle, body: proposalBody });
  const heading = useRef<HTMLHeadingElement>(null);
  const statusRefreshGeneration = useRef(0);
  const commands = useIdempotentCommand(ownerId, [`launch:${launchId}`]);
  const retryBlocked = useRetryDeadline(resource.nextRetryAt);

  useEffect(() => {
    proposalText.current = { title: proposalTitle, body: proposalBody };
  }, [proposalBody, proposalTitle]);

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

  useEffect(
    () => () => {
      statusRefreshGeneration.current += 1;
    },
    [],
  );

  async function capture(): Promise<void> {
    const launch = resource.launch;
    if (
      launch === null ||
      sameLaunchContributionBlock(launch.pull_request, review?.pull_request) !== null
    ) {
      return;
    }
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
    const artifact = review?.artifact ?? launch?.diff;
    const pending = commands.pending?.slot === `launch:branch:${launch?.id}`;
    if (
      launch === null ||
      sameLaunchContributionBlock(launch.pull_request, review?.pull_request) !== null ||
      (!pending && publicationReview === null) ||
      ((artifact === null || artifact === undefined) && !pending)
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
      if (!pending) {
        const latest = await readLaunchDiff(launch.id);
        setReview(latest);
        const latestArtifact = latest.artifact;
        if (
          publicationReview === null ||
          latest.launch_id !== publicationReview.launchId ||
          latest.revision_number !== publicationReview.expectedRevision ||
          latestArtifact === null ||
          latestArtifact.id !== publicationReview.artifactId ||
          latestArtifact.digest !== publicationReview.snapshotDigest ||
          latestArtifact.expected_branch_head !== publicationReview.expectedBranchHead
        ) {
          setPublicationReview(null);
          setReviewFailure(
            'The reviewed artifact changed before publication. Review the current diff and open a new confirmation; CMA did not write to GitHub.',
          );
          return;
        }
      }
      const fallbackRequest: LaunchRevisionDigestRequest = {
        launchId: publicationReview?.launchId ?? launch.id,
        expectedRevision: publicationReview?.expectedRevision ?? launch.revision_number,
        snapshotDigest: publicationReview?.snapshotDigest ?? artifact?.digest ?? '',
      };
      const request = retainedCommandRequest(commands.pending, identity, fallbackRequest, (value) =>
        launchRevisionDigestRequest(value, launch.id),
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
      setPublicationReview(null);
    } catch (cause) {
      if (!commandOutcomeIsUnresolved(cause)) setPublicationReview(null);
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
    const launch = resource.launch;
    if (
      launch === null ||
      sameLaunchContributionBlock(launch.pull_request, review?.pull_request) !== null
    ) {
      return;
    }
    setBusy('proposal');
    setReviewFailure(null);
    setProposalReviewNotice(null);
    try {
      const draft = await reviewPullRequest(launchId, operation);
      setProposalReview({
        draft: freezePullRequestDraft(draft),
        expectedRevision: launch.revision_number,
      });
      setProposalTitle(draft.title);
      setProposalBody(draft.body);
      setProposalMetadataEdited(false);
    } catch (cause) {
      setReviewFailure(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(null);
    }
  }

  async function confirmProposal(): Promise<void> {
    const launch = resource.launch;
    const current = proposal;
    const currentReview = proposalReview;
    const retainedOperation = pendingPullRequestOperation(commands.pending, launchId);
    if (
      launch === null ||
      sameLaunchContributionBlock(launch.pull_request, review?.pull_request) !== null ||
      (current === null && retainedOperation === null)
    ) {
      return;
    }
    setBusy('pr');
    setReviewFailure(null);
    setProposalReviewNotice(null);
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
          edits: retained.updateMetadata ? { title: proposalTitle, body: proposalBody } : {},
        };
      } else {
        if (current === null || currentReview === null) return;
        const displayedText = { title: proposalTitle, body: proposalBody };
        const updateMetadata = current.operation === 'open' || proposalMetadataEdited;
        const edits = updateMetadata ? displayedText : {};
        const sealed = freezePullRequestDraft(
          await reviewPullRequest(launch.id, current.operation, edits),
        );
        const latest = await resource.refresh();
        if (latest === null) {
          setReviewFailure(
            'CMA could not verify the current Launch revision after sealing this proposal. No pull-request write was attempted.',
          );
          return;
        }
        const textChangedWhileSealing =
          proposalText.current.title !== displayedText.title ||
          proposalText.current.body !== displayedText.body;
        const displayedTextChanged =
          updateMetadata &&
          (currentReview.draft.title !== displayedText.title ||
            currentReview.draft.body !== displayedText.body);
        if (
          textChangedWhileSealing ||
          displayedTextChanged ||
          latest.revision_number !== currentReview.expectedRevision ||
          !samePullRequestDraft(currentReview.draft, sealed)
        ) {
          setProposalReview({ draft: sealed, expectedRevision: latest.revision_number });
          setProposalTitle(sealed.title);
          setProposalBody(sealed.body);
          setProposalReviewNotice(PULL_REQUEST_REVIEW_DRIFT_MESSAGE);
          setReviewFailure(PULL_REQUEST_REVIEW_DRIFT_MESSAGE);
          return;
        }
        request = {
          launchId: launch.id,
          operation: sealed.operation,
          expectedRevision: currentReview.expectedRevision,
          snapshotDigest: sealed.intent_digest,
          updateMetadata,
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
            updateMetadata: request.updateMetadata,
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
      setProposalReview(null);
      setProposalReviewNotice(null);
      setProposalMetadataEdited(false);
    } catch (cause) {
      setReviewFailure(
        commandOutcomeIsUnresolved(cause)
          ? 'The pull-request outcome is unknown. CMA retained the command fingerprint and reviewed head. Explicit title or body edits are never stored by this browser.'
          : cause instanceof Error
            ? cause.message
            : String(cause),
      );
      await resource.refresh();
    } finally {
      setBusy(null);
    }
  }

  async function refreshOpenPullRequestStatus(): Promise<void> {
    const launch = resource.launch;
    const currentPullRequest = review?.pull_request ?? launch?.pull_request;
    if (launch === null || currentPullRequest?.state !== 'open') return;
    const generation = statusRefreshGeneration.current + 1;
    statusRefreshGeneration.current = generation;
    setBusy('status');
    setReviewFailure(null);
    setStatusNotice(null);
    const identity = {
      slot: `launch:pull-request-status:${launch.id}`,
      resource: `launch:${launch.id}`,
      action: 'refresh-pull-request-status',
    } as const;
    try {
      const request = retainedCommandRequest(
        commands.pending,
        identity,
        {
          launchId: launch.id,
          expectedRevision: Math.max(launch.revision_number, review?.revision_number ?? 0),
        },
        (value) => pullRequestStatusRefreshRequest(value, launch.id),
      );
      const queued = await commands.execute(
        { ...identity, request, persistRequest: true },
        'pull-request status refresh',
        (idempotencyKey, frozen) =>
          refreshPullRequestStatus(frozen.launchId, frozen.expectedRevision, { idempotencyKey }),
      );
      if (statusRefreshGeneration.current !== generation) return;
      resource.accept(queued);
      setStatusNotice('Pull request status refresh queued. Waiting for a GitHub observation.');
      const previousFingerprint = pullRequestStatusFingerprint(currentPullRequest);
      let readFailed = false;
      for (const delay of PULL_REQUEST_STATUS_POLL_DELAYS_MS) {
        if (delay > 0) await new Promise((resolve) => window.setTimeout(resolve, delay));
        if (statusRefreshGeneration.current !== generation) return;
        try {
          const next = await readLaunchDiff(launch.id);
          if (statusRefreshGeneration.current !== generation) return;
          setReview((current) =>
            current !== null && current.revision_number > next.revision_number ? current : next,
          );
          const observed = next.pull_request;
          if (
            next.revision_number > queued.revision_number &&
            observed !== null &&
            pullRequestStatusFingerprint(observed) !== previousFingerprint
          ) {
            setStatusNotice('Pull request status refreshed from GitHub.');
            return;
          }
        } catch {
          readFailed = true;
        }
      }
      setStatusNotice(
        readFailed
          ? 'The refresh is queued, but its newer observation could not be read yet. CMA will continue the read-only refresh automatically.'
          : 'The refresh is queued. CMA will continue the read-only status check automatically.',
      );
    } catch (cause) {
      setReviewFailure(
        commandOutcomeIsUnresolved(cause)
          ? 'The status-refresh command outcome is unknown. CMA retained the exact command key; reconciling it cannot write to GitHub.'
          : cause instanceof Error
            ? cause.message
            : String(cause),
      );
    } finally {
      if (statusRefreshGeneration.current === generation) setBusy(null);
    }
  }

  async function createAnother(): Promise<void> {
    const launch = resource.launch;
    if (launch === null) return;
    setBusy('new');
    setReviewFailure(null);
    try {
      const identity = {
        slot: `launch:new:${launch.id}`,
        resource: `launch:${launch.id}`,
        action: 'new-launch',
      } as const;
      const request = retainedCommandRequest(
        commands.pending,
        identity,
        { publicId: launch.trigger_public_id },
        (value) => commandObject(value, { publicId: exactString(launch.trigger_public_id) }),
      );
      const next = await commands.execute(
        { ...identity, request, persistRequest: true },
        'New Launch creation',
        (idempotencyKey, frozen) => createNewLaunch(frozen.publicId, { idempotencyKey }),
      );
      setConfirmingNew(false);
      navigate({ kind: 'trigger', publicId: next.trigger_public_id });
    } catch (cause) {
      setReviewFailure(
        commandOutcomeIsUnresolved(cause)
          ? 'The New Launch outcome is unknown. CMA retained the exact command; reconcile it before another Launch action.'
          : cause instanceof Error
            ? cause.message
            : String(cause),
      );
      if (commandOutcomeIsUnresolved(cause)) await resource.refresh();
      else setConfirmingNew(false);
    } finally {
      setBusy(null);
    }
  }

  async function complete(): Promise<void> {
    const launch = resource.launch;
    const slot = `launch:complete:${launch?.id ?? launchId}`;
    const pending = commands.pending?.slot === slot && commands.pending.action === 'complete';
    if (
      launch === null ||
      (!pending && (launch.state !== 'pr_open' || launch.pull_request?.state !== 'open'))
    ) {
      return;
    }
    setBusy('complete');
    setReviewFailure(null);
    try {
      const identity = {
        slot: `launch:complete:${launch.id}`,
        resource: `launch:${launch.id}`,
        action: 'complete',
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
        'CMA work completion',
        (idempotencyKey, frozen) =>
          completeLaunch(frozen.launchId, frozen.expectedRevision, frozen.snapshotDigest, {
            idempotencyKey,
          }),
      );
      resource.accept(next);
      setConfirmingComplete(false);
    } catch (cause) {
      setReviewFailure(
        commandOutcomeIsUnresolved(cause)
          ? 'The completion outcome is unknown. CMA retained the exact command; reconcile it before another Launch action.'
          : cause instanceof Error
            ? cause.message
            : String(cause),
      );
      if (commandOutcomeIsUnresolved(cause)) await resource.refresh();
      else setConfirmingComplete(false);
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
  const newSlot = `launch:new:${launch.id}`;
  const pendingNew = commands.pending?.slot === newSlot && commands.pending.action === 'new-launch';
  const statusSlot = `launch:pull-request-status:${launch.id}`;
  const pendingStatus =
    commands.pending?.slot === statusSlot &&
    commands.pending.action === 'refresh-pull-request-status';
  const completeSlot = `launch:complete:${launch.id}`;
  const pendingComplete =
    commands.pending?.slot === completeSlot && commands.pending.action === 'complete';
  const repository = upstreamRepository(launch);
  const artifact = review?.artifact ?? launch.diff;
  const contribution = review?.contribution ?? launch.contribution;
  const pullRequest = review?.pull_request ?? launch.pull_request;
  const reach = reachState(launch);
  const reachAvailable = reach === 'active';
  const serverStateCurrent = !resource.offline && resource.failure === null;
  const githubRemediation = reachRemediation(reach);
  const reviewRemediation = artifact === null ? null : validationRemediation(artifact);
  const publishRemediation = githubRemediation ?? reviewRemediation;
  const exactReviewContribution =
    artifact !== null && contribution?.diff_artifact_id === artifact.id;
  const exactReviewPublished = exactReviewContribution && contribution?.state === 'published';
  const publicationReason =
    contribution === null
      ? null
      : exactReviewContribution
        ? publicationStatusReason(contribution.state)
        : 'The retained publication intent belongs to a different reviewed artifact. Refresh the Launch before taking another action.';
  const canPublish =
    serverStateCurrent &&
    artifact !== null &&
    contribution === null &&
    reviewRemediation === null &&
    reachAvailable &&
    sameLaunchContributionBlock(launch.pull_request, review?.pull_request) === null;
  const operation = pullRequest === null ? 'open' : pullRequest.state === 'open' ? 'update' : null;
  const pullRequestGuidance =
    pullRequest === null ? null : pullRequestStatusGuidance(pullRequest.state);
  const sameLaunchBlock = sameLaunchContributionBlock(launch.pull_request, review?.pull_request);
  const sameLaunchBlockReason = sameLaunchContributionBlockReason(sameLaunchBlock);
  const statusFailure = pullRequest?.status_failure ?? null;
  const canComplete =
    serverStateCurrent &&
    launch.state === 'pr_open' &&
    launch.pull_request?.state === 'open' &&
    pullRequest?.state === 'open';
  const launchRoute = { kind: 'launch', launchId } as const;
  const recoveryRoute = { kind: 'trigger_recovery', launchId } as const;
  const linkedSessionId = sessionId(launch);
  const linkedSessionRoute =
    linkedSessionId === undefined
      ? null
      : ({ kind: 'session', sessionId: linkedSessionId } as const);
  const canPrepareProposal =
    serverStateCurrent &&
    operation !== null &&
    sameLaunchBlock === null &&
    reachAvailable &&
    reviewRemediation === null &&
    exactReviewPublished;
  const publishReason =
    (!serverStateCurrent
      ? 'Refresh the Launch before starting another GitHub write; the displayed review may be stale.'
      : null) ??
    sameLaunchBlockReason ??
    publishRemediation?.reason ??
    publicationReason ??
    'Publishing writes exactly this reviewed artifact to the guarded Launch branch.';
  const pullRequestReason =
    (!serverStateCurrent
      ? 'Refresh the Launch before starting another GitHub write; the displayed review may be stale.'
      : null) ??
    (sameLaunchBlockReason !== null
      ? sameLaunchBlockReason
      : statusFailure === null
        ? null
        : pullRequestStatusFailureReason(statusFailure)) ??
    publishRemediation?.reason ??
    pullRequestGuidance?.reason ??
    (artifact === null
      ? 'Capture and review a workspace diff before preparing a pull request.'
      : publicationReason !== null
        ? publicationReason
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
            sameLaunchBlock !== null ||
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
      {statusNotice === null ? null : (
        <div className="notice notice--quiet trigger-workflow__notice" role="status">
          <IconRefreshOutline16 />
          <div>
            <p className="notice__title">Pull request status</p>
            <p className="notice__detail">{statusNotice}</p>
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
            <span id="contribution-capture-reason" className="control-reason">
              {sameLaunchBlockReason ??
                'Capturing a diff reads the workspace and writes nothing to GitHub.'}
            </span>
            <Button
              variant="outline"
              aria-describedby="contribution-capture-reason"
              disabled={
                busy !== null ||
                sameLaunchBlock !== null ||
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
                {contribution === null ? null : (
                  <div>
                    <dt>Publication</dt>
                    <dd>{stateLabel(contribution.state)}</dd>
                  </div>
                )}
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
                    onClick={() => {
                      if (artifact === null) return;
                      const expectedRevision =
                        review?.artifact?.id === artifact.id &&
                        review.artifact.digest === artifact.digest
                          ? review.revision_number
                          : launch.revision_number;
                      setPublicationReview({
                        launchId: launch.id,
                        artifactId: artifact.id,
                        expectedRevision,
                        snapshotDigest: artifact.digest,
                        expectedBranchHead: artifact.expected_branch_head,
                      });
                    }}
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
              <div>
                <dt>CMA head binding</dt>
                <dd>
                  {pullRequest.head_matches_launch === null
                    ? 'Not observed'
                    : pullRequest.head_matches_launch
                      ? 'Matches Launch'
                      : 'Changed outside CMA'}
                </dd>
              </div>
              <div>
                <dt>Last provider observation</dt>
                <dd>{timestamp(pullRequest.provider_updated_at)}</dd>
              </div>
              {pullRequest.provider_request_id === null ? null : (
                <div>
                  <dt>Provider request</dt>
                  <dd>
                    <code>{pullRequest.provider_request_id}</code>
                  </dd>
                </div>
              )}
            </dl>
          )}
          {statusFailure === null ? null : (
            <div className="notice notice--quiet" role="status">
              <IconWarningOutline16 />
              <div>
                <p className="notice__title">Pull request status paused by CMA</p>
                <p className="notice__detail">{statusFailure.detail}</p>
                <p className="notice__detail">{statusFailure.remediation}</p>
                {statusFailure.retry_at === undefined ? null : (
                  <p className="notice__detail">Retry after {timestamp(statusFailure.retry_at)}</p>
                )}
                <code>
                  {statusFailure.code} · {statusFailure.correlation_id}
                </code>
              </div>
            </div>
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
                  disabled={
                    resource.loading ||
                    busy !== null ||
                    (pullRequest?.state === 'open' && !reachAvailable) ||
                    (commands.pending !== null && !pendingStatus)
                  }
                  onClick={() => {
                    if (pullRequest?.state === 'open') void refreshOpenPullRequestStatus();
                    else void resource.refresh();
                  }}
                >
                  {pullRequest?.state === 'open' && pendingStatus
                    ? 'Reconcile status refresh'
                    : busy === 'status'
                      ? 'Refreshing status'
                      : 'Refresh status'}
                </Button>
              ) : null}
              {sameLaunchBlock !== null ? (
                <Button
                  variant="primary"
                  disabled={busy !== null || (commands.pending !== null && !pendingNew)}
                  onClick={() => setConfirmingNew(true)}
                >
                  {pendingNew ? 'Reconcile New Launch' : 'New Launch'}
                </Button>
              ) : null}
              {canComplete || pendingComplete ? (
                <Button
                  variant="outline"
                  disabled={busy !== null || (commands.pending !== null && !pendingComplete)}
                  onClick={() => setConfirmingComplete(true)}
                >
                  {pendingComplete ? 'Reconcile completion' : 'Finish CMA work'}
                </Button>
              ) : null}
              {operation === null ? null : (
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
        open={publicationReview !== null || pendingPublish}
        title="Publish this reviewed branch?"
        description="CMA will create one commit with the author and committer shown in this review, then advance only the guarded Launch branch if its head is unchanged."
        onClose={() => {
          if (busy === null && !pendingPublish) setPublicationReview(null);
        }}
        footer={
          <>
            <Button
              disabled={busy !== null || pendingPublish}
              onClick={() => setPublicationReview(null)}
            >
              Cancel
            </Button>
            <Button
              variant="primary"
              disabled={
                !reachAvailable ||
                sameLaunchBlock !== null ||
                busy !== null ||
                (commands.pending !== null && !pendingPublish)
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
      >
        {publicationReview === null ? null : (
          <dl className="launch-facts">
            <div>
              <dt>Artifact</dt>
              <dd>
                <code>{publicationReview.artifactId}</code>
              </dd>
            </div>
            <div>
              <dt>Digest</dt>
              <dd>
                <code>{publicationReview.snapshotDigest}</code>
              </dd>
            </div>
            <div>
              <dt>Launch revision</dt>
              <dd>{String(publicationReview.expectedRevision)}</dd>
            </div>
            <div>
              <dt>Expected head</dt>
              <dd>
                <code>{shortRevision(publicationReview.expectedBranchHead)}</code>
              </dd>
            </div>
          </dl>
        )}
      </Modal>

      <Modal
        open={confirmingComplete || pendingComplete}
        title="Finish CMA work on this pull request?"
        description="The pull request remains open on GitHub for maintainer review. CMA stops this Launch's automation, starts bounded revocation of its remaining GitHub reach, and no further same-Launch updates can be published."
        onClose={() => {
          if (busy === null && !pendingComplete) setConfirmingComplete(false);
        }}
        footer={
          <>
            <Button
              disabled={busy !== null || pendingComplete}
              onClick={() => setConfirmingComplete(false)}
            >
              Keep working
            </Button>
            <Button
              variant="primary"
              disabled={busy !== null || (commands.pending !== null && !pendingComplete)}
              onClick={() => void complete()}
            >
              {busy === 'complete'
                ? pendingComplete
                  ? 'Reconciling'
                  : 'Submitting'
                : pendingComplete
                  ? 'Reconcile completion'
                  : 'Finish CMA work'}
            </Button>
          </>
        }
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

      <Modal
        open={proposal !== null || pendingProposal}
        title={`${proposalOperation === 'update' ? 'Update' : 'Open'} pull request?`}
        description={
          proposalOperation === 'update'
            ? proposalMetadataEdited
              ? 'This advances the pull request to the exact reviewed head and applies your explicit metadata edits only if the provider metadata is unchanged.'
              : 'This advances the pull request to the exact reviewed head without including its provider title or body in the update.'
            : "This writes the exact reviewed head to the upstream repository's pull-request list. Review and merge remain on GitHub."
        }
        onClose={() => {
          if (busy === null && !pendingProposal) {
            setProposalReview(null);
            setProposalReviewNotice(null);
            setProposalMetadataEdited(false);
          }
        }}
        footer={
          <>
            <Button
              disabled={busy !== null || pendingProposal}
              onClick={() => {
                setProposalReview(null);
                setProposalReviewNotice(null);
                setProposalMetadataEdited(false);
              }}
            >
              Cancel
            </Button>
            <Button
              variant="primary"
              disabled={
                !reachAvailable ||
                sameLaunchBlock !== null ||
                busy !== null ||
                ((proposalOperation === 'open' || proposalMetadataEdited) &&
                  proposalTitle.trim() === '') ||
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
          {proposalReviewNotice === null ? null : (
            <p className="notice notice--quiet" role="status">
              {proposalReviewNotice}
            </p>
          )}
          {proposalOperation === 'update' ? (
            <p className="notice notice--quiet">
              {proposalMetadataEdited
                ? 'Your explicit metadata edit is applied only if the observed provider title and body are unchanged.'
                : 'Provider title and body are shown for context and are not included in this head-only update.'}
            </p>
          ) : null}
          <label className="field">
            <span className="field__label">Title</span>
            <Input
              placement="field__input"
              value={proposalTitle}
              disabled={
                busy === 'pr' ||
                (pendingProposal && !commands.pending?.requiresRequestReconstruction)
              }
              onChange={(event) => {
                setProposalMetadataEdited(true);
                setProposalTitle(event.target.value);
              }}
            />
          </label>
          <label className="field">
            <span className="field__label">Body</span>
            <textarea
              className="field__input"
              value={proposalBody}
              disabled={
                busy === 'pr' ||
                (pendingProposal && !commands.pending?.requiresRequestReconstruction)
              }
              onChange={(event) => {
                setProposalMetadataEdited(true);
                setProposalBody(event.target.value);
              }}
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
              <dt>Base revision</dt>
              <dd>
                <code>
                  {proposal === null ? '' : shortRevision(proposal.expected_base_revision)}
                </code>
              </dd>
            </div>
            <div>
              <dt>Head revision</dt>
              <dd>
                <code>
                  {proposal === null ? '' : shortRevision(proposal.expected_head_revision)}
                </code>
              </dd>
            </div>
            <div>
              <dt>Intent</dt>
              <dd>
                <code>{proposal === null ? '' : shortRevision(proposal.intent_digest)}</code>
              </dd>
            </div>
          </dl>
        </div>
      </Modal>
    </main>
  );
}
