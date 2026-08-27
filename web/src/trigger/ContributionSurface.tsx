import { useEffect, useRef, useState } from 'react';
import type { DiffFile, DiffReview, PullRequestDraft } from '@chronoai/cma-sdk';
import {
  TransportError,
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
  IconRefreshOutline16,
  IconRightUpOutline16,
  IconWarningOutline16,
} from '../ui/icons';
import {
  mergeabilityLabel,
  reachState,
  shortRevision,
  stateLabel,
  timestamp,
  upstreamRepository,
} from './model';
import { useLaunchResource } from './useLaunchResource';
import { useIdempotentCommand } from './useIdempotentCommand';
import './ContributionSurface.css';

interface ContributionSurfaceProps {
  readonly launchId: string;
  readonly navigate: AppRouter['navigate'];
}

type Busy = 'capture' | 'publish' | 'proposal' | 'pr' | 'download' | null;

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

function downloadableName(launchId: string, artifactId: string): string {
  return `cma-${launchId}-${artifactId}.json`;
}

export function ContributionSurface({ launchId, navigate }: ContributionSurfaceProps) {
  const resource = useLaunchResource(launchId);
  const [review, setReview] = useState<DiffReview | null>(null);
  const [reviewFailure, setReviewFailure] = useState<string | null>(null);
  const [busy, setBusy] = useState<Busy>(null);
  const [confirmingPublish, setConfirmingPublish] = useState(false);
  const [proposal, setProposal] = useState<PullRequestDraft | null>(null);
  const [proposalTitle, setProposalTitle] = useState('');
  const [proposalBody, setProposalBody] = useState('');
  const heading = useRef<HTMLHeadingElement>(null);
  const commands = useIdempotentCommand();

  useEffect(() => {
    heading.current?.focus();
  }, [launchId]);

  useEffect(() => {
    const controller = new AbortController();
    void readLaunchDiff(launchId, controller.signal)
      .then((next) => {
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
      const next =
        commands.pending?.slot === slot
          ? await commands.reconcile<typeof launch>(slot, 'diff capture')
          : await commands.execute(slot, 'diff capture', (idempotencyKey) =>
              requestLaunchDiff(launch.id, launch.revision_number, launch.effective.digest, {
                idempotencyKey,
              }),
            );
      resource.accept(next);
    } catch (cause) {
      setReviewFailure(
        cause instanceof TransportError
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
    if (launch === null || artifact === null || artifact === undefined) return;
    setBusy('publish');
    setReviewFailure(null);
    const slot = `launch:branch:${launch.id}`;
    try {
      const next =
        commands.pending?.slot === slot
          ? await commands.reconcile<typeof launch>(slot, 'branch publication')
          : await commands.execute(slot, 'branch publication', (idempotencyKey) =>
              publishLaunchBranch(launch.id, launch.revision_number, artifact.digest, {
                idempotencyKey,
              }),
            );
      resource.accept(next);
      setConfirmingPublish(false);
    } catch (cause) {
      if (!(cause instanceof TransportError)) setConfirmingPublish(false);
      setReviewFailure(
        cause instanceof TransportError
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
    if (launch === null || current === null) return;
    setBusy('pr');
    setReviewFailure(null);
    const slot = `launch:pull-request:${launch.id}:${current.operation}`;
    try {
      let next;
      if (commands.pending?.slot === slot) {
        next = await commands.reconcile<typeof launch>(slot, 'pull-request confirmation');
      } else {
        const edits = { title: proposalTitle, body: proposalBody };
        const sealed = await reviewPullRequest(launch.id, current.operation, edits);
        next = await commands.execute(slot, 'pull-request confirmation', (idempotencyKey) =>
          confirmPullRequest(
            launch.id,
            sealed.operation,
            launch.revision_number,
            sealed.intent_digest,
            edits,
            { idempotencyKey },
          ),
        );
      }
      resource.accept(next);
      setProposal(null);
    } catch (cause) {
      setReviewFailure(
        cause instanceof TransportError
          ? 'The pull-request outcome is unknown. CMA retained the exact reviewed title, body, head, and command for reconciliation.'
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
  const proposalSlot =
    proposal === null ? null : `launch:pull-request:${launch.id}:${proposal.operation}`;
  const pendingCapture = commands.pending?.slot === captureSlot;
  const pendingPublish = commands.pending?.slot === publishSlot;
  const pendingProposal = proposalSlot !== null && commands.pending?.slot === proposalSlot;
  const repository = upstreamRepository(launch);
  const artifact = review?.artifact ?? launch.diff;
  const contribution = review?.contribution ?? launch.contribution;
  const pullRequest = review?.pull_request ?? launch.pull_request;
  const expectedHeadMatches = artifact?.expected_branch_head === artifact?.observed_branch_head;
  const withinLimit =
    artifact !== null && artifact.total_changed_files <= artifact.changed_file_limit;
  const validationPassed =
    artifact?.validation === 'valid' && artifact.checks.every((check) => check.passed);
  const reach = reachState(launch);
  const reachAvailable =
    reach === undefined || !['revoked', 'revocation_pending', 'expired'].includes(reach);
  const canPublish =
    artifact !== null &&
    artifact.total_changed_files > 0 &&
    expectedHeadMatches &&
    withinLimit &&
    validationPassed &&
    reachAvailable &&
    contribution?.diff_artifact_id !== artifact.id;
  const operation = pullRequest === null ? 'open' : pullRequest.state === 'open' ? 'update' : null;
  const launchRoute = { kind: 'launch', launchId } as const;

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
                <span className="control-reason">
                  {canPublish
                    ? 'Publishing writes exactly this reviewed artifact to the guarded Launch branch.'
                    : artifact.total_changed_files === 0
                      ? 'There are no changes to publish.'
                      : !expectedHeadMatches
                        ? 'The branch head changed. Capture and review a new diff.'
                        : !withinLimit
                          ? 'This diff exceeds the changed-file limit.'
                          : !validationPassed
                            ? 'Validation must pass before publication.'
                            : !reachAvailable
                              ? 'GitHub reach must be restored before publication.'
                              : 'This reviewed artifact is already published.'}
                </span>
                <Button
                  variant="primary"
                  disabled={!canPublish || busy !== null || commands.pending !== null}
                  onClick={() => setConfirmingPublish(true)}
                >
                  Publish branch
                </Button>
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
            <span className="control-reason">
              {contribution === null
                ? 'Publish the reviewed branch before preparing a pull request.'
                : operation === null
                  ? pullRequest?.state === 'closed_unmerged'
                    ? 'This pull request is closed. Create a New Launch for a separate attempt.'
                    : 'A merged pull request cannot be updated from CMA.'
                  : `${operation === 'open' ? 'Opening' : 'Updating'} remains a separate confirmation.`}
            </span>
            {operation === null ? null : (
              <Button
                variant="primary"
                disabled={contribution === null || busy !== null || commands.pending !== null}
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
        </section>
      </div>

      <Modal
        open={confirmingPublish}
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
              disabled={busy !== null || (commands.pending !== null && !pendingPublish)}
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
        open={proposal !== null}
        title={`${proposal?.operation === 'update' ? 'Update' : 'Open'} pull request?`}
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
                  : proposal?.operation === 'update'
                    ? 'Update pull request'
                    : 'Open pull request'}
            </Button>
          </>
        }
      >
        <div className="contribution-proposal">
          {proposal?.operation === 'update' ? (
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
              disabled={pendingProposal}
              onChange={(event) => setProposalTitle(event.target.value)}
            />
          </label>
          <label className="field">
            <span className="field__label">Body</span>
            <textarea
              className="field__input"
              value={proposalBody}
              disabled={pendingProposal}
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
