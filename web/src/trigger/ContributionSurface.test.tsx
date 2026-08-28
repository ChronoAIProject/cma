import { readFileSync } from 'node:fs';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type {
  DiffArtifact,
  DiffReview,
  DiffValidationOutcome,
  Launch,
  LaunchCapabilityRevocationState,
  LaunchReachStatus,
  PullRequestDraft,
  PullRequestRecord,
} from '@chronoai/cma-sdk';
import { TransportError } from '../api/client';
import { ContributionSurface } from './ContributionSurface';

const api = vi.hoisted(() => ({
  completeLaunch: vi.fn(),
  confirmPullRequest: vi.fn(),
  createNewLaunch: vi.fn(),
  downloadDiffArtifact: vi.fn(),
  publishLaunchBranch: vi.fn(),
  readLaunchDiff: vi.fn(),
  refreshPullRequestStatus: vi.fn(),
  requestLaunchDiff: vi.fn(),
  reviewPullRequest: vi.fn(),
}));

const launchResource = vi.hoisted(() => ({ use: vi.fn() }));

vi.mock('../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../api/client')>()),
  ...api,
}));

vi.mock('./useLaunchResource', () => ({
  useLaunchResource: launchResource.use,
}));

const LAUNCH = JSON.parse(
  readFileSync('../sdk/typescript/tests/fixtures/launch.json', 'utf8'),
) as Launch;
const DIFF_WIRE = JSON.parse(
  readFileSync('../sdk/typescript/tests/fixtures/diff_review.json', 'utf8'),
) as DiffReview;
// Most command tests start after branch publication but before any PR exists.
// Keep that state explicit now that the shared wire fixture exercises a live PR.
const DIFF = { ...DIFF_WIRE, pull_request: null } satisfies DiffReview;
const PROPOSAL = JSON.parse(
  readFileSync('../sdk/typescript/tests/fixtures/pull_request_draft.json', 'utf8'),
) as PullRequestDraft;

function sealProposalEdits(edits: { readonly title?: string; readonly body?: string }) {
  const title = edits.title ?? PROPOSAL.title;
  const body = edits.body ?? PROPOSAL.body;
  if (title === PROPOSAL.title && body === PROPOSAL.body) return PROPOSAL;
  return {
    ...PROPOSAL,
    title,
    body,
    body_digest: 'b'.repeat(64),
    intent_digest: 'c'.repeat(64),
  } satisfies PullRequestDraft;
}

function reach(state: LaunchCapabilityRevocationState): LaunchReachStatus {
  return {
    service_id: LAUNCH.connection?.service_id ?? 'service-fixture',
    state,
    policy_generation: 1,
    policy_fence: 1,
    runtime_audience: 'runtime-fixture',
    expires_at_epoch_seconds: 1_800_000_000,
    numeric_binding_observed_at: '2026-08-27T00:00:00Z',
    refreshed_at: '2026-08-27T00:00:00Z',
    tombstone_acknowledged: false,
    agent_key_revocation_confirmed: false,
  };
}

function readyLaunch(reachState: LaunchCapabilityRevocationState | null = 'active'): Launch {
  return {
    ...LAUNCH,
    state: 'ready',
    session: {
      session_id: 'sesn_trigger',
      environment_profile_id: 'env_trigger',
      runtime_generation: 1,
      created_at: LAUNCH.created_at,
    },
    reach: reachState === null ? null : reach(reachState),
  };
}

function pullRequestOpenLaunch(): Launch {
  return {
    ...readyLaunch(),
    state: 'pr_open',
    pull_request: openPullRequest(),
  };
}

function openPullRequest(): PullRequestRecord {
  return {
    ...PROPOSAL,
    provider_id: 101,
    provider_node_id: 'PR_provider_node',
    number: 17,
    canonical_url: 'https://github.com/upstream-owner/project/pull/17',
    state: 'open',
    base_revision: PROPOSAL.expected_base_revision,
    head_revision: PROPOSAL.expected_head_revision,
    observed_title: PROPOSAL.title,
    observed_body_digest: PROPOSAL.body_digest,
    mergeability: 'computing',
    checks: 'pending',
    confirmation_snapshot_digest: PROPOSAL.intent_digest,
    confirmed_at: '2026-08-27T00:00:15Z',
    created_at: '2026-08-27T00:00:15Z',
    updated_at: '2026-08-27T00:00:15Z',
    provider_created_at: '2026-08-27T00:00:15Z',
    provider_updated_at: '2026-08-27T00:00:15Z',
    status_failure: null,
    provider_request_id: 'github-request-17',
    head_matches_launch: true,
  };
}

function pullRequestWithState(state: PullRequestRecord['state']): PullRequestRecord {
  const record = openPullRequest();
  if (state === 'creating') {
    return {
      ...record,
      state,
      provider_id: null,
      provider_node_id: null,
      number: null,
      canonical_url: null,
      base_revision: null,
      head_revision: null,
      observed_title: null,
      observed_body_digest: null,
      provider_created_at: null,
      provider_updated_at: null,
    };
  }
  return { ...record, state };
}

function reviewedDiff(launchId: string, revision: number, path: string): DiffReview {
  const artifact = DIFF.artifact;
  const file = artifact?.files[0];
  if (artifact === null || file === undefined) throw new Error('diff fixture omitted its file');
  return {
    ...DIFF,
    launch_id: launchId,
    revision_number: revision,
    artifact: { ...artifact, files: [{ ...file, path }] },
  };
}

function reviewOutcome(outcome: DiffValidationOutcome): DiffReview {
  const artifact = DIFF.artifact;
  if (artifact === null) throw new Error('diff fixture omitted its artifact');
  let overrides: Partial<DiffArtifact> = {};
  if (outcome === 'stale_head') {
    overrides = { observed_branch_head: 'f'.repeat(40) };
  } else if (outcome === 'too_many_changes') {
    overrides = { total_changed_files: artifact.changed_file_limit + 1 };
  } else if (
    outcome === 'no_changes' ||
    outcome === 'unsupported_file' ||
    outcome === 'missing_baseline' ||
    outcome === 'corrupt_checkpoint' ||
    outcome === 'artifact_too_large'
  ) {
    overrides = { total_changed_files: 0, changed_bytes: 0, files: [] };
  }
  return {
    ...DIFF,
    artifact: { ...artifact, ...overrides, validation: outcome },
    contribution: null,
    pull_request: null,
  };
}

function publicationDrift(field: 'identity' | 'digest' | 'revision' | 'head'): DiffReview {
  const artifact = DIFF.artifact;
  if (artifact === null) throw new Error('diff fixture omitted its artifact');
  return {
    ...DIFF,
    revision_number: field === 'revision' ? DIFF.revision_number + 1 : DIFF.revision_number,
    artifact: {
      ...artifact,
      id: field === 'identity' ? '99999999-9999-4999-8999-999999999999' : artifact.id,
      digest: field === 'digest' ? '9'.repeat(64) : artifact.digest,
      expected_branch_head: field === 'head' ? '8'.repeat(40) : artifact.expected_branch_head,
    },
    contribution: null,
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

function first<T>(items: readonly T[]): T {
  const item = items[0];
  if (item === undefined) throw new Error('expected at least one matching element');
  return item;
}

function storageText(): string {
  return Array.from({ length: window.localStorage.length }, (_, index) => {
    const key = window.localStorage.key(index);
    return key === null ? '' : (window.localStorage.getItem(key) ?? '');
  }).join('\n');
}

function resource(launch = readyLaunch()) {
  return {
    launch,
    loading: false,
    failure: null,
    failureDisposition: null,
    nextRetryAt: null,
    offline: false,
    lastUpdated: Date.now(),
    liveSummary: 'Launch ready',
    refresh: vi.fn().mockResolvedValue(launch),
    accept: vi.fn(),
  };
}

beforeEach(() => {
  vi.clearAllMocks();
  launchResource.use.mockReturnValue(resource());
  api.readLaunchDiff.mockResolvedValue({ ...DIFF, artifact: null, contribution: null });
  api.reviewPullRequest.mockImplementation(async (_launchId, _operation, edits = {}) =>
    sealProposalEdits(edits),
  );
});

describe('contribution command reconciliation', () => {
  it('projects a historical contribution as retained read-only evidence', async () => {
    if (LAUNCH.runtime_admission === null) throw new Error('fixture omitted runtime admission');
    const legacy: Launch = {
      ...readyLaunch(),
      state: 'pr_open',
      runtime_admission: { ...LAUNCH.runtime_admission, kernel: 'codex' },
      diff: DIFF.artifact,
      contribution: DIFF.contribution,
      pull_request: openPullRequest(),
    };
    launchResource.use.mockReturnValue(resource(legacy));
    api.readLaunchDiff.mockResolvedValue({
      ...DIFF,
      launch_id: legacy.id,
      revision_number: legacy.revision_number,
      state: legacy.state,
      artifact: legacy.diff,
      contribution: legacy.contribution,
      pull_request: legacy.pull_request,
    });

    render(<ContributionSurface ownerId="test-owner" launchId={legacy.id} navigate={vi.fn()} />);

    expect(await screen.findByText('Historical contribution is read-only')).toBeInTheDocument();
    expect(screen.getByText(/Codex \(historical, read-only\)/)).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Open session' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Download artifact' })).toBeEnabled();
    expect(screen.getByRole('button', { name: 'New Launch' })).toBeEnabled();
    for (const name of [
      'Review diff',
      'Refresh diff',
      'Publish branch',
      'Refresh status',
      'Open pull request',
      'Update pull request',
      'Finish CMA work',
    ]) {
      expect(screen.queryByRole('button', { name })).toBeNull();
    }
    expect(api.requestLaunchDiff).not.toHaveBeenCalled();
    expect(api.publishLaunchBranch).not.toHaveBeenCalled();
    expect(api.reviewPullRequest).not.toHaveBeenCalled();
    expect(api.confirmPullRequest).not.toHaveBeenCalled();
    expect(api.refreshPullRequestStatus).not.toHaveBeenCalled();
    expect(api.completeLaunch).not.toHaveBeenCalled();
  });

  it('clears A review state and accepts a lower-revision B review', async () => {
    const launchA = { ...readyLaunch(), id: 'launch-a', revision_number: 10 };
    const launchB = { ...readyLaunch(), id: 'launch-b', revision_number: 1 };
    const reviewA = reviewedDiff(launchA.id, 10, 'A-only.txt');
    const reviewB = reviewedDiff(launchB.id, 1, 'B-only.txt');
    const pendingB = deferred<DiffReview>();
    launchResource.use.mockImplementation((id: string) =>
      resource(id === launchA.id ? launchA : launchB),
    );
    api.readLaunchDiff.mockImplementation((id: string) =>
      id === launchA.id ? Promise.resolve(reviewA) : pendingB.promise,
    );
    const view = render(
      <ContributionSurface ownerId="test-owner" launchId={launchA.id} navigate={vi.fn()} />,
    );

    expect(await screen.findByText('A-only.txt')).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Open pull request' }));
    expect(await screen.findByRole('dialog', { name: 'Open pull request?' })).toBeInTheDocument();
    const aSignal = api.readLaunchDiff.mock.calls[0]?.[1] as AbortSignal;

    view.rerender(
      <ContributionSurface ownerId="test-owner" launchId={launchB.id} navigate={vi.fn()} />,
    );
    expect(aSignal.aborted).toBe(true);
    expect(screen.queryByText('A-only.txt')).toBeNull();
    expect(screen.queryByRole('dialog')).toBeNull();

    await act(async () => {
      pendingB.resolve(reviewB);
    });
    expect(await screen.findByText('B-only.txt')).toBeInTheDocument();
    expect(screen.queryByText('A-only.txt')).toBeNull();
  });

  it('clears an A diff failure when B succeeds', async () => {
    const launchA = { ...readyLaunch(), id: 'launch-a' };
    const launchB = { ...readyLaunch(), id: 'launch-b' };
    const reviewB = reviewedDiff(launchB.id, 1, 'B-after-failure.txt');
    launchResource.use.mockImplementation((id: string) =>
      resource(id === launchA.id ? launchA : launchB),
    );
    api.readLaunchDiff.mockImplementation((id: string) =>
      id === launchA.id
        ? Promise.reject(new Error('A diff unavailable'))
        : Promise.resolve(reviewB),
    );
    const view = render(
      <ContributionSurface ownerId="test-owner" launchId={launchA.id} navigate={vi.fn()} />,
    );

    expect(await screen.findByRole('alert')).toHaveTextContent('A diff unavailable');
    view.rerender(
      <ContributionSurface ownerId="test-owner" launchId={launchB.id} navigate={vi.fn()} />,
    );

    expect(screen.queryByText('A diff unavailable')).toBeNull();
    expect(await screen.findByText('B-after-failure.txt')).toBeInTheDocument();
  });

  it('ignores a late A diff after B is current', async () => {
    const launchA = { ...readyLaunch(), id: 'launch-a' };
    const launchB = { ...readyLaunch(), id: 'launch-b' };
    const pendingA = deferred<DiffReview>();
    const reviewB = reviewedDiff(launchB.id, 1, 'B-current.txt');
    launchResource.use.mockImplementation((id: string) =>
      resource(id === launchA.id ? launchA : launchB),
    );
    api.readLaunchDiff.mockImplementation((id: string) =>
      id === launchA.id ? pendingA.promise : Promise.resolve(reviewB),
    );
    const view = render(
      <ContributionSurface ownerId="test-owner" launchId={launchA.id} navigate={vi.fn()} />,
    );
    const aSignal = api.readLaunchDiff.mock.calls[0]?.[1] as AbortSignal;

    view.rerender(
      <ContributionSurface ownerId="test-owner" launchId={launchB.id} navigate={vi.fn()} />,
    );
    expect(aSignal.aborted).toBe(true);
    expect(await screen.findByText('B-current.txt')).toBeInTheDocument();
    await act(async () => {
      pendingA.resolve(reviewedDiff(launchA.id, 99, 'late-A.txt'));
    });

    expect(screen.queryByText('late-A.txt')).toBeNull();
    expect(screen.getByText('B-current.txt')).toBeInTheDocument();
  });

  it.each([
    [
      'missing',
      null,
      false,
      'CMA has no GitHub reach status for this Launch. Select an eligible GitHub connection before continuing.',
      'Review GitHub access',
    ],
    ['active', 'active', true, null, null],
    [
      'revocation pending',
      'revocation_pending',
      false,
      'GitHub reach revocation is still converging. Writes stay disabled until CMA observes a tombstone acknowledgement, confirmed agent-key revocation, or bounded expiry.',
      'Check reach status',
    ],
    [
      'revoked',
      'revoked',
      false,
      'GitHub reach for this Launch was revoked. Reauthorize or select another eligible GitHub connection before continuing.',
      'Review GitHub access',
    ],
    [
      'expired',
      'expired',
      false,
      'GitHub reach for this Launch expired. Reauthorize or select another eligible GitHub connection before continuing.',
      'Review GitHub access',
    ],
  ] as const)(
    'gates publish and PR open/update when GitHub reach is %s',
    async (_label, reachState, available, reason, action) => {
      launchResource.use.mockReturnValue(resource(readyLaunch(reachState)));
      api.readLaunchDiff.mockResolvedValue({ ...DIFF, contribution: null, pull_request: null });
      const publishView = render(
        <ContributionSurface ownerId="test-owner" launchId={LAUNCH.id} navigate={vi.fn()} />,
      );

      const publish = await screen.findByRole('button', { name: 'Publish branch' });
      if (available) {
        expect(publish).toBeEnabled();
      } else {
        expect(publish).toBeDisabled();
        expect(reason).not.toBeNull();
        expect(screen.getAllByText(reason)).toHaveLength(2);
        expect(
          screen.getAllByRole(action === 'Check reach status' ? 'button' : 'link', {
            name: action ?? '',
          }),
        ).toHaveLength(2);
      }
      publishView.unmount();

      api.readLaunchDiff.mockResolvedValue(DIFF);
      const openView = render(
        <ContributionSurface ownerId="test-owner" launchId={LAUNCH.id} navigate={vi.fn()} />,
      );
      const open = await screen.findByRole('button', { name: 'Open pull request' });
      if (available) {
        expect(open).toBeEnabled();
      } else {
        expect(open).toBeDisabled();
        expect(screen.getAllByText(reason)).toHaveLength(2);
      }
      openView.unmount();

      api.readLaunchDiff.mockResolvedValue({ ...DIFF, pull_request: openPullRequest() });
      render(<ContributionSurface ownerId="test-owner" launchId={LAUNCH.id} navigate={vi.fn()} />);
      const update = await screen.findByRole('button', { name: 'Update pull request' });
      if (available) {
        expect(update).toBeEnabled();
      } else {
        expect(update).toBeDisabled();
        expect(screen.getAllByText(reason)).toHaveLength(2);
      }
    },
  );

  it('omits provider metadata from an ordinary head-only PR update', async () => {
    const providerRecord = {
      ...openPullRequest(),
      observed_title: 'Maintainer-retitled contribution',
      observed_body_digest: '9'.repeat(64),
    };
    const updateDraft = {
      ...PROPOSAL,
      operation: 'update',
      previous_head_revision: providerRecord.head_revision,
      target_number: providerRecord.number,
      target_provider_id: providerRecord.provider_id,
      target_provider_node_id: providerRecord.provider_node_id,
      update_metadata: false,
      expected_previous_title: null,
      expected_previous_body_digest: null,
      intent_digest: '8'.repeat(64),
    } satisfies PullRequestDraft;
    api.readLaunchDiff.mockResolvedValue({ ...DIFF, pull_request: providerRecord });
    api.reviewPullRequest.mockResolvedValue(updateDraft);
    api.confirmPullRequest.mockResolvedValue({ ...readyLaunch(), revision_number: 1 });
    render(<ContributionSurface ownerId="test-owner" launchId={LAUNCH.id} navigate={vi.fn()} />);
    const user = userEvent.setup();

    await user.click(await screen.findByRole('button', { name: 'Update pull request' }));
    const dialog = await screen.findByRole('dialog', { name: 'Update pull request?' });
    expect(dialog).toHaveTextContent('without including its provider title or body in the update');
    await user.click(within(dialog).getByRole('button', { name: 'Update pull request' }));

    await waitFor(() => expect(api.confirmPullRequest).toHaveBeenCalledOnce());
    expect(api.reviewPullRequest).toHaveBeenNthCalledWith(1, LAUNCH.id, 'update');
    expect(api.reviewPullRequest).toHaveBeenNthCalledWith(2, LAUNCH.id, 'update', {});
    expect(api.confirmPullRequest).toHaveBeenCalledWith(
      LAUNCH.id,
      'update',
      LAUNCH.revision_number,
      updateDraft.intent_digest,
      {},
      expect.objectContaining({ idempotencyKey: expect.stringMatching(/^cma-web-/) }),
    );
    expect(api.confirmPullRequest).not.toHaveBeenCalledWith(
      expect.anything(),
      expect.anything(),
      expect.anything(),
      expect.anything(),
      expect.objectContaining({ title: providerRecord.observed_title }),
      expect.anything(),
    );
  });

  it('retains explicit PR metadata edits behind provider-metadata CAS', async () => {
    const providerRecord = {
      ...openPullRequest(),
      observed_title: 'Maintainer-retitled contribution',
      observed_body_digest: '9'.repeat(64),
    };
    const baseUpdateDraft = {
      ...PROPOSAL,
      operation: 'update',
      previous_head_revision: providerRecord.head_revision,
      target_number: providerRecord.number,
      target_provider_id: providerRecord.provider_id,
      target_provider_node_id: providerRecord.provider_node_id,
      update_metadata: false,
      expected_previous_title: null,
      expected_previous_body_digest: null,
      intent_digest: '8'.repeat(64),
    } satisfies PullRequestDraft;
    const editedTitle = 'Contributor-requested title';
    const editedDraft = {
      ...baseUpdateDraft,
      update_metadata: true,
      expected_previous_title: providerRecord.observed_title,
      expected_previous_body_digest: providerRecord.observed_body_digest,
      title: editedTitle,
      intent_digest: '7'.repeat(64),
    } satisfies PullRequestDraft;
    api.readLaunchDiff.mockResolvedValue({ ...DIFF, pull_request: providerRecord });
    api.reviewPullRequest.mockImplementation(async (_id, _operation, edits = {}) =>
      edits.title === editedTitle ? editedDraft : baseUpdateDraft,
    );
    api.confirmPullRequest.mockResolvedValue({ ...readyLaunch(), revision_number: 1 });
    render(<ContributionSurface ownerId="test-owner" launchId={LAUNCH.id} navigate={vi.fn()} />);
    const user = userEvent.setup();

    await user.click(await screen.findByRole('button', { name: 'Update pull request' }));
    const dialog = await screen.findByRole('dialog', { name: 'Update pull request?' });
    const title = within(dialog).getByLabelText('Title');
    await user.clear(title);
    await user.type(title, editedTitle);
    expect(dialog).toHaveTextContent('applies your explicit metadata edits');

    await user.click(within(dialog).getByRole('button', { name: 'Update pull request' }));
    expect(await screen.findByRole('alert')).toHaveTextContent(
      /proposal changed while CMA resealed it/i,
    );
    expect(api.confirmPullRequest).not.toHaveBeenCalled();

    await user.click(within(dialog).getByRole('button', { name: 'Update pull request' }));
    await waitFor(() => expect(api.confirmPullRequest).toHaveBeenCalledOnce());
    expect(api.reviewPullRequest).toHaveBeenLastCalledWith(LAUNCH.id, 'update', {
      title: editedTitle,
      body: PROPOSAL.body,
    });
    expect(api.confirmPullRequest).toHaveBeenCalledWith(
      LAUNCH.id,
      'update',
      LAUNCH.revision_number,
      editedDraft.intent_digest,
      { title: editedTitle, body: PROPOSAL.body },
      expect.objectContaining({ idempotencyKey: expect.stringMatching(/^cma-web-/) }),
    );
    expect(editedDraft.expected_previous_title).toBe(providerRecord.observed_title);
    expect(editedDraft.expected_previous_body_digest).toBe(providerRecord.observed_body_digest);
  });

  it('queues an open-PR status read and waits for a newer provider observation', async () => {
    const pullRequest = openPullRequest();
    const currentLaunch = {
      ...readyLaunch(),
      state: 'pr_open' as const,
      pull_request: pullRequest,
    };
    const current = resource(currentLaunch);
    const queued = { ...currentLaunch, revision_number: currentLaunch.revision_number + 1 };
    const observed = {
      ...DIFF,
      revision_number: queued.revision_number + 1,
      pull_request: {
        ...pullRequest,
        checks: 'passing' as const,
        mergeability: 'mergeable' as const,
        updated_at: '2026-08-27T00:01:00Z',
        provider_updated_at: '2026-08-27T00:01:00Z',
        provider_request_id: 'github-request-refreshed',
      },
    };
    launchResource.use.mockReturnValue(current);
    api.refreshPullRequestStatus.mockResolvedValue(queued);
    api.readLaunchDiff
      .mockResolvedValueOnce({ ...DIFF, pull_request: pullRequest })
      .mockResolvedValue(observed);
    render(<ContributionSurface ownerId="test-owner" launchId={LAUNCH.id} navigate={vi.fn()} />);

    await userEvent.click(await screen.findByRole('button', { name: 'Refresh status' }));

    await waitFor(() => expect(api.refreshPullRequestStatus).toHaveBeenCalledOnce());
    expect(api.refreshPullRequestStatus).toHaveBeenCalledWith(
      LAUNCH.id,
      currentLaunch.revision_number,
      expect.objectContaining({ idempotencyKey: expect.stringMatching(/^cma-web-/) }),
    );
    await waitFor(() => expect(current.accept).toHaveBeenCalledWith(queued));
    expect(
      await screen.findByText('Pull request status refreshed from GitHub.'),
    ).toBeInTheDocument();
    expect(screen.getByText('github-request-refreshed')).toBeInTheDocument();
    expect(screen.getByText('passing')).toBeInTheDocument();
    expect(screen.getByText('Mergeable')).toBeInTheDocument();
    expect(api.reviewPullRequest).not.toHaveBeenCalled();
    expect(api.confirmPullRequest).not.toHaveBeenCalled();
  });

  it('keeps a manually changed PR readable and refreshable while disabling CMA Update', async () => {
    api.readLaunchDiff.mockResolvedValue({
      ...DIFF,
      contribution: null,
      pull_request: { ...openPullRequest(), head_matches_launch: false },
    });
    render(<ContributionSurface ownerId="test-owner" launchId={LAUNCH.id} navigate={vi.fn()} />);

    const update = await screen.findByRole('button', { name: 'Update pull request' });
    expect(update).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Refresh status' })).toBeEnabled();
    expect(screen.getByRole('button', { name: 'New Launch' })).toBeEnabled();
    expect(screen.getByRole('button', { name: 'Refresh diff' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Publish branch' })).toBeDisabled();
    expect(screen.getByRole('link', { name: `#17 ${PROPOSAL.title}` })).toHaveAttribute(
      'href',
      openPullRequest().canonical_url,
    );
    expect(screen.getByText('Changed outside CMA')).toBeInTheDocument();
    expect(
      screen.getAllByText(/head changed outside CMA.*same Launch cannot capture another diff/i)[0],
    ).toBeInTheDocument();
    expect(api.requestLaunchDiff).not.toHaveBeenCalled();
    expect(api.publishLaunchBranch).not.toHaveBeenCalled();
  });

  it('shows a safe provider status failure, retry deadline, and correlation identifiers', async () => {
    api.readLaunchDiff.mockResolvedValue({
      ...DIFF,
      pull_request: {
        ...openPullRequest(),
        provider_request_id: 'github-request-delayed',
        status_failure: {
          class: 'retryable_transient',
          code: 'github_status_rate_limited',
          detail: 'GitHub delayed the read-only status check.',
          remediation: 'CMA will retry without issuing a pull-request write.',
          correlation_id: 'correlation-status-17',
          retry_at: '2026-08-27T00:05:00Z',
          observed_at: '2026-08-27T00:00:00Z',
        },
      },
    });
    render(<ContributionSurface ownerId="test-owner" launchId={LAUNCH.id} navigate={vi.fn()} />);

    expect(await screen.findByText('Pull request status paused by CMA')).toBeInTheDocument();
    expect(
      screen.getAllByText('GitHub delayed the read-only status check.').length,
    ).toBeGreaterThan(0);
    expect(screen.getByText(/Retry after/)).toBeInTheDocument();
    expect(
      screen.getByText(/github_status_rate_limited.*correlation-status-17/),
    ).toBeInTheDocument();
    expect(screen.getByText('github-request-delayed')).toBeInTheDocument();
  });

  it.each([
    [
      'intent_recorded',
      /Publication intent is recorded, but Git object writes are not yet confirmed/i,
    ],
    [
      'objects_written',
      /Git objects are written, but the guarded Launch branch has not been advanced/i,
    ],
    [
      'ref_update_pending',
      /guarded branch update may have reached GitHub.*awaiting read-only reconciliation/i,
    ],
    ['conflict', /guarded Launch branch changed before publication completed/i],
    ['recovery_required', /publication outcome is ambiguous.*paused automatic writes/i],
    ['failed', /Branch publication failed.*does not claim a published result/i],
  ] as const)(
    'does not claim or use an exact-artifact %s publication as published',
    async (state, reason) => {
      if (DIFF.contribution === null) throw new Error('diff fixture omitted its contribution');
      api.readLaunchDiff.mockResolvedValue({
        ...DIFF,
        contribution: { ...DIFF.contribution, state },
        pull_request: null,
      });
      render(<ContributionSurface ownerId="test-owner" launchId={LAUNCH.id} navigate={vi.fn()} />);

      expect(await screen.findByText(state.replaceAll('_', ' '))).toBeInTheDocument();
      expect(screen.getByRole('button', { name: 'Publish branch' })).toBeDisabled();
      expect(screen.getByRole('button', { name: 'Open pull request' })).toBeDisabled();
      expect(screen.getAllByText(reason)).toHaveLength(2);
      expect(api.publishLaunchBranch).not.toHaveBeenCalled();
      expect(api.reviewPullRequest).not.toHaveBeenCalled();
    },
  );

  it.each([
    ['missing', null],
    ['revoked', 'revoked'],
    ['expired', 'expired'],
  ] as const)('routes %s GitHub reach to the retained Launch recovery', async (_label, state) => {
    const navigate = vi.fn();
    launchResource.use.mockReturnValue(resource(readyLaunch(state)));
    api.readLaunchDiff.mockResolvedValue(DIFF);
    render(<ContributionSurface ownerId="test-owner" launchId={LAUNCH.id} navigate={navigate} />);

    await userEvent.click(
      first(await screen.findAllByRole('link', { name: 'Review GitHub access' })),
    );
    expect(navigate).toHaveBeenCalledWith({ kind: 'trigger_recovery', launchId: LAUNCH.id });
  });

  it('refreshes server truth while revocation is still converging', async () => {
    const current = resource(readyLaunch('revocation_pending'));
    launchResource.use.mockReturnValue(current);
    api.readLaunchDiff.mockResolvedValue(DIFF);
    render(<ContributionSurface ownerId="test-owner" launchId={LAUNCH.id} navigate={vi.fn()} />);

    await userEvent.click(
      first(await screen.findAllByRole('button', { name: 'Check reach status' })),
    );
    expect(current.refresh).toHaveBeenCalledOnce();
  });

  it.each([
    ['creating', /creation is still reconciling.*will not open a second pull request/i],
    ['updating', /update is still reconciling.*will not issue a second update/i],
    [
      'recovery_required',
      /outcome is ambiguous and requires provider reconciliation.*will not issue another pull request write/i,
    ],
  ] as const)(
    'refreshes server truth without another provider write while a PR is %s',
    async (state, reason) => {
      const current = resource();
      launchResource.use.mockReturnValue(current);
      api.readLaunchDiff.mockResolvedValue({
        ...DIFF,
        pull_request: pullRequestWithState(state),
      });
      render(<ContributionSurface ownerId="test-owner" launchId={LAUNCH.id} navigate={vi.fn()} />);

      expect(await screen.findByText(reason)).toBeInTheDocument();
      expect(screen.queryByRole('button', { name: 'Open pull request' })).toBeNull();
      expect(screen.queryByRole('button', { name: 'Update pull request' })).toBeNull();
      await userEvent.click(screen.getByRole('button', { name: 'Refresh status' }));

      expect(current.refresh).toHaveBeenCalledOnce();
      expect(api.reviewPullRequest).not.toHaveBeenCalled();
      expect(api.confirmPullRequest).not.toHaveBeenCalled();
    },
  );

  it('creates a separate Launch from a closed pull request without mutating the old PR', async () => {
    const navigate = vi.fn();
    const next = {
      ...readyLaunch(),
      id: 'new-launch-id',
      trigger_public_id: LAUNCH.trigger_public_id,
    };
    api.createNewLaunch.mockResolvedValue(next);
    api.readLaunchDiff.mockResolvedValue({
      ...DIFF,
      pull_request: pullRequestWithState('closed_unmerged'),
    });
    render(<ContributionSurface ownerId="test-owner" launchId={LAUNCH.id} navigate={navigate} />);

    await userEvent.click(await screen.findByRole('button', { name: 'New Launch' }));
    const dialog = screen.getByRole('dialog', { name: 'Create a separate Launch?' });
    await userEvent.click(within(dialog).getByRole('button', { name: 'Create New Launch' }));

    await waitFor(() => expect(api.createNewLaunch).toHaveBeenCalledOnce());
    expect(api.createNewLaunch).toHaveBeenCalledWith(
      LAUNCH.trigger_public_id,
      expect.objectContaining({ idempotencyKey: expect.stringMatching(/^cma-web-/) }),
    );
    await waitFor(() =>
      expect(navigate).toHaveBeenCalledWith({
        kind: 'trigger',
        publicId: LAUNCH.trigger_public_id,
      }),
    );
    expect(api.reviewPullRequest).not.toHaveBeenCalled();
    expect(api.confirmPullRequest).not.toHaveBeenCalled();
  });

  it('finishes CMA orchestration while leaving the handed-off pull request open', async () => {
    const launch = pullRequestOpenLaunch();
    const completed = {
      ...launch,
      state: 'completed',
      revision_number: launch.revision_number + 1,
    } satisfies Launch;
    const current = resource(launch);
    launchResource.use.mockReturnValue(current);
    api.readLaunchDiff.mockResolvedValue({
      ...DIFF,
      revision_number: launch.revision_number,
      pull_request: launch.pull_request,
    });
    api.completeLaunch.mockResolvedValue(completed);
    render(<ContributionSurface ownerId="test-owner" launchId={launch.id} navigate={vi.fn()} />);
    const user = userEvent.setup();

    await user.click(await screen.findByRole('button', { name: 'Finish CMA work' }));
    const dialog = screen.getByRole('dialog', {
      name: 'Finish CMA work on this pull request?',
    });
    expect(dialog).toHaveTextContent('pull request remains open on GitHub');
    expect(dialog).toHaveTextContent('no further same-Launch updates');
    await user.click(within(dialog).getByRole('button', { name: 'Finish CMA work' }));

    await waitFor(() => expect(api.completeLaunch).toHaveBeenCalledOnce());
    expect(api.completeLaunch).toHaveBeenCalledWith(
      launch.id,
      launch.revision_number,
      launch.effective.digest,
      expect.objectContaining({ idempotencyKey: expect.stringMatching(/^cma-web-/) }),
    );
    await waitFor(() => expect(current.accept).toHaveBeenCalledWith(completed));
    expect(completed.pull_request?.state).toBe('open');
  });

  it('reconciles an ambiguous completion with the original command key', async () => {
    const launch = pullRequestOpenLaunch();
    const current = resource(launch);
    launchResource.use.mockReturnValue(current);
    api.readLaunchDiff.mockResolvedValue({
      ...DIFF,
      revision_number: launch.revision_number,
      pull_request: launch.pull_request,
    });
    api.completeLaunch.mockRejectedValue(new TransportError('response lost'));
    render(<ContributionSurface ownerId="test-owner" launchId={launch.id} navigate={vi.fn()} />);
    const user = userEvent.setup();

    await user.click(await screen.findByRole('button', { name: 'Finish CMA work' }));
    const dialog = screen.getByRole('dialog', {
      name: 'Finish CMA work on this pull request?',
    });
    await user.click(within(dialog).getByRole('button', { name: 'Finish CMA work' }));
    await waitFor(() => expect(api.completeLaunch).toHaveBeenCalledTimes(2));
    expect(api.completeLaunch.mock.calls[0]?.[3].idempotencyKey).toBe(
      api.completeLaunch.mock.calls[1]?.[3].idempotencyKey,
    );

    api.completeLaunch.mockResolvedValue({
      ...launch,
      state: 'completed',
      revision_number: launch.revision_number + 1,
    });
    const reconcile = await within(dialog).findByRole('button', {
      name: 'Reconcile completion',
    });
    await user.click(reconcile);
    await waitFor(() => expect(api.completeLaunch).toHaveBeenCalledTimes(3));
    expect(new Set(api.completeLaunch.mock.calls.map((call) => call[3].idempotencyKey)).size).toBe(
      1,
    );
  });

  it.each([
    [
      'the Launch has not reached the open-PR state',
      { ...pullRequestOpenLaunch(), state: 'ready' } satisfies Launch,
      false,
    ],
    [
      'the provider pull request is closed',
      {
        ...pullRequestOpenLaunch(),
        pull_request: pullRequestWithState('closed_unmerged'),
      } satisfies Launch,
      false,
    ],
    ['the displayed Launch is offline', pullRequestOpenLaunch(), true],
  ] as const)('withholds completion when %s', async (_reason, launch, offline) => {
    launchResource.use.mockReturnValue({ ...resource(launch), offline });
    api.readLaunchDiff.mockResolvedValue({
      ...DIFF,
      revision_number: launch.revision_number,
      pull_request: launch.pull_request,
    });

    render(<ContributionSurface ownerId="test-owner" launchId={launch.id} navigate={vi.fn()} />);

    await screen.findByText('Pull request');
    expect(screen.queryByRole('button', { name: 'Finish CMA work' })).toBeNull();
    expect(api.completeLaunch).not.toHaveBeenCalled();
  });

  it.each([
    ['closed_unmerged', /pull request is closed.*same Launch cannot capture another diff/i],
    ['merged', /pull request was merged.*same Launch cannot capture another diff/i],
  ] as const)(
    'keeps terminal %s PR history readable and disables every same-Launch contribution action',
    async (state, reason) => {
      api.readLaunchDiff.mockResolvedValue({
        ...DIFF,
        contribution: null,
        pull_request: pullRequestWithState(state),
      });
      render(<ContributionSurface ownerId="test-owner" launchId={LAUNCH.id} navigate={vi.fn()} />);

      expect(first(await screen.findAllByText(reason))).toBeInTheDocument();
      expect(screen.getByRole('link', { name: `#17 ${PROPOSAL.title}` })).toHaveAttribute(
        'href',
        openPullRequest().canonical_url,
      );
      expect(screen.getByRole('button', { name: 'Refresh diff' })).toBeDisabled();
      expect(screen.getByRole('button', { name: 'Publish branch' })).toBeDisabled();
      expect(screen.queryByRole('button', { name: 'Open pull request' })).toBeNull();
      expect(screen.queryByRole('button', { name: 'Update pull request' })).toBeNull();
      expect(screen.queryByRole('button', { name: 'Refresh status' })).toBeNull();
      expect(screen.getByRole('button', { name: 'New Launch' })).toBeEnabled();
      expect(api.requestLaunchDiff).not.toHaveBeenCalled();
      expect(api.publishLaunchBranch).not.toHaveBeenCalled();
      expect(api.reviewPullRequest).not.toHaveBeenCalled();
      expect(api.confirmPullRequest).not.toHaveBeenCalled();
    },
  );

  it.each([
    ['no_changes', /No workspace changes were found/, 'link', 'Open session'],
    ['stale_head', /branch head changed after this diff was sealed/, 'button', 'Recapture diff'],
    [
      'workflow_scope_required',
      /Workflow-file publication is not authorized/,
      'link',
      'Open session',
    ],
    ['too_many_changes', /above the .*file limit/, 'link', 'Open session'],
    ['too_many_bytes', /changed-byte limit/, 'link', 'Open session'],
    ['unsupported_file', /unsupported file type/, 'link', 'Open session'],
    [
      'missing_baseline',
      /could not read the sealed workspace baseline/,
      'button',
      'Retry diff capture',
    ],
    ['corrupt_checkpoint', /checkpoint could not be verified/, 'button', 'Recapture diff'],
    ['artifact_too_large', /safe size limit/, 'link', 'Open session'],
  ] as const)(
    'names and remediates the %s diff outcome without enabling a GitHub write',
    async (outcome, reason, actionRole, actionName) => {
      api.readLaunchDiff.mockResolvedValue(reviewOutcome(outcome));
      render(<ContributionSurface ownerId="test-owner" launchId={LAUNCH.id} navigate={vi.fn()} />);

      const publish = await screen.findByRole('button', { name: 'Publish branch' });
      const open = screen.getByRole('button', { name: 'Open pull request' });
      expect(publish).toBeDisabled();
      expect(open).toBeDisabled();
      expect(publish).toHaveAttribute('aria-describedby', 'contribution-publish-reason');
      expect(open).toHaveAttribute('aria-describedby', 'contribution-pr-reason');
      expect(screen.getAllByText(reason)).toHaveLength(2);
      expect(screen.getAllByRole(actionRole, { name: actionName })).toHaveLength(2);
      expect(api.publishLaunchBranch).not.toHaveBeenCalled();
      expect(api.reviewPullRequest).not.toHaveBeenCalled();
    },
  );

  it('recaptures a stale diff through the existing idempotent capture command', async () => {
    const current = resource();
    launchResource.use.mockReturnValue(current);
    api.readLaunchDiff.mockResolvedValue(reviewOutcome('stale_head'));
    api.requestLaunchDiff.mockResolvedValue({ ...readyLaunch(), revision_number: 1 });
    render(<ContributionSurface ownerId="test-owner" launchId={LAUNCH.id} navigate={vi.fn()} />);

    await userEvent.click(first(await screen.findAllByRole('button', { name: 'Recapture diff' })));
    await waitFor(() => expect(api.requestLaunchDiff).toHaveBeenCalledOnce());
    expect(api.requestLaunchDiff).toHaveBeenCalledWith(
      LAUNCH.id,
      LAUNCH.revision_number,
      LAUNCH.effective.digest,
      expect.objectContaining({ idempotencyKey: expect.stringMatching(/^cma-web-/) }),
    );
    await waitFor(() => expect(current.accept).toHaveBeenCalledOnce());
  });

  it('routes file-fix outcomes back to the exact linked session', async () => {
    const navigate = vi.fn();
    api.readLaunchDiff.mockResolvedValue(reviewOutcome('unsupported_file'));
    render(<ContributionSurface ownerId="test-owner" launchId={LAUNCH.id} navigate={navigate} />);

    await userEvent.click(first(await screen.findAllByRole('link', { name: 'Open session' })));
    expect(navigate).toHaveBeenCalledWith({ kind: 'session', sessionId: 'sesn_trigger' });
  });

  it('shows the exact failed check and keeps provider writes disabled', async () => {
    const artifact = DIFF.artifact;
    if (artifact === null) throw new Error('diff fixture omitted its artifact');
    api.readLaunchDiff.mockResolvedValue({
      ...DIFF,
      contribution: null,
      pull_request: null,
      artifact: {
        ...artifact,
        validation: 'valid',
        checks: [{ key: 'path-policy', passed: false, summary: 'Generated files are blocked.' }],
      },
    });
    render(<ContributionSurface ownerId="test-owner" launchId={LAUNCH.id} navigate={vi.fn()} />);

    expect(await screen.findByRole('button', { name: 'Publish branch' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Open pull request' })).toBeDisabled();
    expect(
      screen.getAllByText(
        'Required check path-policy failed: Generated files are blocked. Fix the reported workspace state, then refresh the diff.',
      ),
    ).toHaveLength(2);
    expect(api.publishLaunchBranch).not.toHaveBeenCalled();
    expect(api.reviewPullRequest).not.toHaveBeenCalled();
  });

  it('discloses an online refresh failure and retries the retained Launch read', async () => {
    const current = {
      ...resource(),
      failure: 'CMA returned a provider failure.',
      failureDisposition: 'retryable' as const,
    };
    launchResource.use.mockReturnValue(current);
    api.readLaunchDiff.mockResolvedValue(DIFF);
    render(<ContributionSurface ownerId="test-owner" launchId={LAUNCH.id} navigate={vi.fn()} />);

    expect(screen.getByRole('alert')).toHaveTextContent('Contribution data may be stale.');
    expect(await screen.findByRole('button', { name: 'Open pull request' })).toBeDisabled();
    expect(
      screen.getByText(DIFF.artifact?.files[0]?.path ?? 'missing reviewed file'),
    ).toBeVisible();
    expect(
      screen.getAllByText(
        'Refresh the Launch before starting another GitHub write; the displayed review may be stale.',
      ),
    ).toHaveLength(2);
    await userEvent.click(screen.getByRole('button', { name: 'Retry refresh' }));
    expect(current.refresh).toHaveBeenCalledOnce();
  });

  it.each([
    ['failed refresh', { failure: 'CMA returned a provider failure.' }],
    ['offline cache', { offline: true }],
  ] as const)(
    'withholds new publish, PR-open, and PR-update intent while %s data is displayed',
    async (_label, resourceState) => {
      const staleResource = { ...resource(), ...resourceState };
      launchResource.use.mockReturnValue(staleResource);

      api.readLaunchDiff.mockResolvedValueOnce({
        ...DIFF,
        contribution: null,
        pull_request: null,
      });
      const publish = render(
        <ContributionSurface ownerId="test-owner" launchId={LAUNCH.id} navigate={vi.fn()} />,
      );
      expect(await screen.findByRole('button', { name: 'Publish branch' })).toBeDisabled();
      publish.unmount();

      api.readLaunchDiff.mockResolvedValueOnce({ ...DIFF, pull_request: null });
      const open = render(
        <ContributionSurface ownerId="test-owner" launchId={LAUNCH.id} navigate={vi.fn()} />,
      );
      expect(await screen.findByRole('button', { name: 'Open pull request' })).toBeDisabled();
      open.unmount();

      api.readLaunchDiff.mockResolvedValueOnce({ ...DIFF, pull_request: openPullRequest() });
      render(<ContributionSurface ownerId="test-owner" launchId={LAUNCH.id} navigate={vi.fn()} />);
      expect(await screen.findByRole('button', { name: 'Update pull request' })).toBeDisabled();
    },
  );

  it('replays diff capture with one key and no second logical command', async () => {
    const currentResource = resource();
    launchResource.use.mockReturnValue(currentResource);
    api.requestLaunchDiff.mockRejectedValue(new TransportError('response lost'));
    render(<ContributionSurface ownerId="test-owner" launchId={LAUNCH.id} navigate={vi.fn()} />);
    const user = userEvent.setup();

    await user.click(await screen.findByRole('button', { name: 'Review diff' }));
    await waitFor(() => expect(api.requestLaunchDiff).toHaveBeenCalledTimes(2));
    expect(api.requestLaunchDiff.mock.calls[0]?.[3].idempotencyKey).toBe(
      api.requestLaunchDiff.mock.calls[1]?.[3].idempotencyKey,
    );

    api.requestLaunchDiff.mockResolvedValue({ ...readyLaunch(), revision_number: 1 });
    await user.click(await screen.findByRole('button', { name: 'Reconcile diff capture' }));
    await waitFor(() => expect(api.requestLaunchDiff).toHaveBeenCalledTimes(3));
    expect(
      new Set(api.requestLaunchDiff.mock.calls.map((call) => call[3].idempotencyKey)).size,
    ).toBe(1);
    await waitFor(() => expect(currentResource.accept).toHaveBeenCalledOnce());
  });

  it('keeps the reviewed artifact confirmation while branch publication is ambiguous', async () => {
    api.readLaunchDiff.mockResolvedValue({ ...DIFF, contribution: null });
    api.publishLaunchBranch.mockRejectedValue(new TransportError('response lost'));
    render(<ContributionSurface ownerId="test-owner" launchId={LAUNCH.id} navigate={vi.fn()} />);
    const user = userEvent.setup();

    await user.click(await screen.findByRole('button', { name: 'Publish branch' }));
    const dialog = screen.getByRole('dialog', { name: 'Publish this reviewed branch?' });
    await user.click(within(dialog).getByRole('button', { name: 'Publish branch' }));
    await waitFor(() => expect(api.publishLaunchBranch).toHaveBeenCalledTimes(2));
    await waitFor(() =>
      expect(
        within(dialog).getByRole('button', { name: 'Reconcile branch publication' }),
      ).toBeEnabled(),
    );

    api.publishLaunchBranch.mockResolvedValue({ ...readyLaunch(), revision_number: 1 });
    await user.click(within(dialog).getByRole('button', { name: 'Reconcile branch publication' }));
    await waitFor(() => expect(api.publishLaunchBranch).toHaveBeenCalledTimes(3));
    expect(
      new Set(api.publishLaunchBranch.mock.calls.map((call) => call[3].idempotencyKey)).size,
    ).toBe(1);
  });

  it.each(['identity', 'digest', 'revision', 'head'] as const)(
    'invalidates branch confirmation when the reviewed artifact %s drifts during preflight',
    async (field) => {
      api.readLaunchDiff.mockResolvedValue({ ...DIFF, contribution: null });
      render(<ContributionSurface ownerId="test-owner" launchId={LAUNCH.id} navigate={vi.fn()} />);
      const user = userEvent.setup();

      await user.click(await screen.findByRole('button', { name: 'Publish branch' }));
      const dialog = screen.getByRole('dialog', { name: 'Publish this reviewed branch?' });
      expect(dialog).toHaveTextContent(DIFF.artifact?.id ?? 'missing artifact');
      expect(dialog).toHaveTextContent(`Launch revision${String(DIFF.revision_number)}`);

      const preflight = deferred<DiffReview>();
      api.readLaunchDiff.mockReturnValueOnce(preflight.promise);
      await user.click(within(dialog).getByRole('button', { name: 'Publish branch' }));
      await waitFor(() => expect(api.readLaunchDiff).toHaveBeenCalledTimes(2));
      await act(async () => {
        preflight.resolve(publicationDrift(field));
      });

      expect(await screen.findByRole('alert')).toHaveTextContent(
        /reviewed artifact changed before publication.*did not write to GitHub/i,
      );
      expect(screen.queryByRole('dialog', { name: 'Publish this reviewed branch?' })).toBeNull();
      expect(api.publishLaunchBranch).not.toHaveBeenCalled();
    },
  );

  it('stops for a newly displayed PR review when any sealed authority drifts', async () => {
    const resealed = {
      ...PROPOSAL,
      base_ref: 'release-candidate',
      expected_base_revision: '8'.repeat(40),
      expected_head_revision: '9'.repeat(40),
      intent_digest: 'a'.repeat(64),
    } satisfies PullRequestDraft;
    api.readLaunchDiff.mockResolvedValue(DIFF);
    render(<ContributionSurface ownerId="test-owner" launchId={LAUNCH.id} navigate={vi.fn()} />);
    const user = userEvent.setup();

    await user.click(await screen.findByRole('button', { name: 'Open pull request' }));
    const dialog = await screen.findByRole('dialog', { name: 'Open pull request?' });
    api.reviewPullRequest.mockResolvedValueOnce(resealed);
    await user.click(within(dialog).getByRole('button', { name: 'Open pull request' }));

    expect(await screen.findByRole('alert')).toHaveTextContent(
      /proposal changed while CMA resealed it.*did not write to GitHub/i,
    );
    expect(within(dialog).getByRole('status')).toHaveTextContent(
      /review the updated target, revisions, content, and intent/i,
    );
    expect(api.confirmPullRequest).not.toHaveBeenCalled();
    expect(within(dialog).getByText('release-candidate')).toBeInTheDocument();
    expect(within(dialog).getByText('999999999999')).toBeInTheDocument();

    api.reviewPullRequest.mockResolvedValueOnce(resealed);
    await user.click(within(dialog).getByRole('button', { name: 'Open pull request' }));
    await waitFor(() => expect(api.confirmPullRequest).toHaveBeenCalledOnce());
    expect(api.confirmPullRequest).toHaveBeenCalledWith(
      LAUNCH.id,
      'open',
      LAUNCH.revision_number,
      resealed.intent_digest,
      { title: resealed.title, body: resealed.body },
      expect.objectContaining({ idempotencyKey: expect.stringMatching(/^cma-web-/) }),
    );
  });

  it('requires a new PR review when the Launch revision advances during resealing', async () => {
    const current = resource();
    const advanced = { ...readyLaunch(), revision_number: LAUNCH.revision_number + 1 };
    current.refresh.mockResolvedValue(advanced);
    launchResource.use.mockReturnValue(current);
    api.readLaunchDiff.mockResolvedValue(DIFF);
    render(<ContributionSurface ownerId="test-owner" launchId={LAUNCH.id} navigate={vi.fn()} />);
    const user = userEvent.setup();

    await user.click(await screen.findByRole('button', { name: 'Open pull request' }));
    const dialog = await screen.findByRole('dialog', { name: 'Open pull request?' });
    await user.click(within(dialog).getByRole('button', { name: 'Open pull request' }));

    expect(await screen.findByRole('alert')).toHaveTextContent(
      /proposal changed while CMA resealed it.*did not write to GitHub/i,
    );
    expect(api.confirmPullRequest).not.toHaveBeenCalled();

    await user.click(within(dialog).getByRole('button', { name: 'Open pull request' }));
    await waitFor(() => expect(api.confirmPullRequest).toHaveBeenCalledOnce());
    expect(api.confirmPullRequest.mock.calls[0]?.[2]).toBe(advanced.revision_number);
  });

  it('freezes edited PR intent and retains the dialog until an ambiguous confirmation reconciles', async () => {
    api.readLaunchDiff.mockResolvedValue(DIFF);
    api.confirmPullRequest.mockRejectedValue(new TransportError('response lost'));
    render(<ContributionSurface ownerId="test-owner" launchId={LAUNCH.id} navigate={vi.fn()} />);
    const user = userEvent.setup();

    await user.click(await screen.findByRole('button', { name: 'Open pull request' }));
    const dialog = await screen.findByRole('dialog', { name: 'Open pull request?' });
    const title = within(dialog).getByLabelText('Title');
    await user.clear(title);
    await user.type(title, 'My retained PR title');
    await user.click(within(dialog).getByRole('button', { name: 'Open pull request' }));

    await waitFor(() => expect(api.reviewPullRequest).toHaveBeenCalledTimes(2));
    expect(api.confirmPullRequest).not.toHaveBeenCalled();
    expect(await screen.findByRole('alert')).toHaveTextContent(
      /proposal changed while CMA resealed it.*did not write to GitHub/i,
    );
    expect(title).toHaveValue('My retained PR title');

    await user.click(within(dialog).getByRole('button', { name: 'Open pull request' }));
    await waitFor(() => expect(api.reviewPullRequest).toHaveBeenCalledTimes(3));
    await waitFor(() => expect(api.confirmPullRequest).toHaveBeenCalledTimes(2));
    expect(await screen.findByRole('alert')).toHaveTextContent(/pull-request outcome is unknown/i);
    expect(title).toHaveValue('My retained PR title');
    expect(title).toBeDisabled();
    expect(within(dialog).getByRole('button', { name: 'Reconcile pull request' })).toBeEnabled();
    expect(api.confirmPullRequest.mock.calls[0]?.[4]).toEqual({
      title: 'My retained PR title',
      body: PROPOSAL.body,
    });

    api.confirmPullRequest.mockResolvedValue({ ...readyLaunch(), revision_number: 1 });
    await user.click(within(dialog).getByRole('button', { name: 'Reconcile pull request' }));
    await waitFor(() => expect(api.confirmPullRequest).toHaveBeenCalledTimes(3));

    expect(api.reviewPullRequest).toHaveBeenCalledTimes(3);
    expect(api.confirmPullRequest.mock.calls[2]?.[4]).toEqual(
      api.confirmPullRequest.mock.calls[0]?.[4],
    );
    expect(
      new Set(api.confirmPullRequest.mock.calls.map((call) => call[5].idempotencyKey)).size,
    ).toBe(1);
  });

  it('reconstructs private PR text after reload only with the exact request and original key', async () => {
    const privateTitle = 'Private PR title reconstructed after reload';
    const privateBody = 'Private PR body that must never enter local storage';
    api.readLaunchDiff.mockResolvedValue(DIFF);
    api.confirmPullRequest.mockRejectedValue(new TransportError('response lost'));
    const first = render(
      <ContributionSurface ownerId="test-owner" launchId={LAUNCH.id} navigate={vi.fn()} />,
    );
    const user = userEvent.setup();

    await user.click(await screen.findByRole('button', { name: 'Open pull request' }));
    const firstDialog = await screen.findByRole('dialog', { name: 'Open pull request?' });
    const firstTitle = within(firstDialog).getByLabelText('Title');
    const firstBody = within(firstDialog).getByLabelText('Body');
    await user.clear(firstTitle);
    await user.type(firstTitle, privateTitle);
    await user.clear(firstBody);
    await user.type(firstBody, privateBody);
    await user.click(within(firstDialog).getByRole('button', { name: 'Open pull request' }));

    expect(await screen.findByRole('alert')).toHaveTextContent(
      /proposal changed while CMA resealed it.*did not write to GitHub/i,
    );
    expect(api.confirmPullRequest).not.toHaveBeenCalled();
    await user.click(within(firstDialog).getByRole('button', { name: 'Open pull request' }));
    await waitFor(() => expect(api.confirmPullRequest).toHaveBeenCalledTimes(2));
    const originalKey = api.confirmPullRequest.mock.calls[0]?.[5].idempotencyKey as string;
    expect(storageText()).not.toContain(privateTitle);
    expect(storageText()).not.toContain(privateBody);
    first.unmount();

    render(<ContributionSurface ownerId="test-owner" launchId={LAUNCH.id} navigate={vi.fn()} />);
    const dialog = await screen.findByRole('dialog', { name: 'Open pull request?' });
    const title = within(dialog).getByLabelText('Title');
    const body = within(dialog).getByLabelText('Body');
    await waitFor(() => expect(title).toBeEnabled());
    expect(title).toHaveValue('');
    expect(body).toHaveValue('');

    await user.type(title, 'Changed PR title');
    await user.type(body, 'Changed PR body');
    await user.click(within(dialog).getByRole('button', { name: 'Reconcile pull request' }));
    expect(await screen.findByRole('alert')).toHaveTextContent(/private text/i);
    expect(api.confirmPullRequest).toHaveBeenCalledTimes(2);

    api.confirmPullRequest.mockResolvedValue({ ...readyLaunch(), revision_number: 1 });
    await user.clear(title);
    await user.type(title, privateTitle);
    await user.clear(body);
    await user.type(body, privateBody);
    await user.click(within(dialog).getByRole('button', { name: 'Reconcile pull request' }));

    await waitFor(() => expect(api.confirmPullRequest).toHaveBeenCalledTimes(3));
    expect(api.confirmPullRequest.mock.calls[2]?.[5].idempotencyKey).toBe(originalKey);
    expect(api.confirmPullRequest.mock.calls[2]?.[4]).toEqual({
      title: privateTitle,
      body: privateBody,
    });
  });

  it('renders authoritative line statistics when the summary omits its patch', async () => {
    const artifact = DIFF.artifact;
    const file = artifact?.files[0];
    if (artifact === null || file === undefined) throw new Error('diff fixture omitted its file');
    api.readLaunchDiff.mockResolvedValue({
      ...DIFF,
      artifact: {
        ...artifact,
        files: [
          {
            ...file,
            additions: 12,
            deletions: 4,
            patch: undefined,
            patch_complete: false,
          },
        ],
      },
    });

    const view = render(
      <ContributionSurface ownerId="test-owner" launchId={LAUNCH.id} navigate={vi.fn()} />,
    );

    expect(await screen.findByText('+12 −4')).toBeVisible();
    expect(screen.queryByText('Line changes unavailable')).toBeNull();
    expect(view.container.querySelector('.contribution-file pre')).toBeNull();
  });

  it('names unavailable binary line statistics without fabricating zeroes', async () => {
    const artifact = DIFF.artifact;
    const file = artifact?.files[0];
    if (artifact === null || file === undefined) throw new Error('diff fixture omitted its file');
    api.readLaunchDiff.mockResolvedValue({
      ...DIFF,
      artifact: {
        ...artifact,
        files: [
          {
            ...file,
            binary: true,
            additions: null,
            deletions: null,
            patch: undefined,
            patch_complete: false,
          },
        ],
      },
    });

    render(<ContributionSurface ownerId="test-owner" launchId={LAUNCH.id} navigate={vi.fn()} />);

    expect(await screen.findByText('Line changes unavailable')).toBeVisible();
    expect(screen.queryByText('+0 −0')).toBeNull();
  });

  it('renders hostile filenames and patches as inert text', async () => {
    const artifact = DIFF.artifact;
    const file = artifact?.files[0];
    if (artifact === null || file === undefined) throw new Error('diff fixture omitted its file');
    const hostilePath =
      'src/</span><img data-cma-trigger-image src=x onerror="window.cmaOwned=true">.tsx';
    const hostilePatch = [
      '--- baseline',
      '+++ checkpoint',
      '@@ -0,0 +1 @@',
      '+</pre><script data-cma-trigger-script>window.cmaOwned=true</script><img src=x onerror=alert(1)>',
    ].join('\n');
    api.readLaunchDiff.mockResolvedValue({
      ...DIFF,
      artifact: {
        ...artifact,
        files: [{ ...file, path: hostilePath, patch: hostilePatch }],
      },
    });

    const view = render(
      <ContributionSurface ownerId="test-owner" launchId={LAUNCH.id} navigate={vi.fn()} />,
    );

    const path = await screen.findByText(hostilePath);
    expect(path.tagName).toBe('SPAN');
    const patch = view.container.querySelector('.contribution-file pre');
    expect(patch).not.toBeNull();
    expect(patch).toHaveTextContent(hostilePatch, { normalizeWhitespace: false });
    expect(view.container.querySelector('script')).toBeNull();
    expect(view.container.querySelector('img')).toBeNull();
    expect(view.container.querySelector('[onerror]')).toBeNull();
    expect(view.container.querySelector('[data-cma-trigger-script]')).toBeNull();
    expect(view.container.querySelector('[data-cma-trigger-image]')).toBeNull();
  });
});
