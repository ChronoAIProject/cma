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
  confirmPullRequest: vi.fn(),
  downloadDiffArtifact: vi.fn(),
  publishLaunchBranch: vi.fn(),
  readLaunchDiff: vi.fn(),
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
const DIFF = JSON.parse(
  readFileSync('../sdk/typescript/tests/fixtures/diff_review.json', 'utf8'),
) as DiffReview;
const PROPOSAL = JSON.parse(
  readFileSync('../sdk/typescript/tests/fixtures/pull_request_draft.json', 'utf8'),
) as PullRequestDraft;

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
  api.reviewPullRequest.mockResolvedValue(PROPOSAL);
});

describe('contribution command reconciliation', () => {
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

  it.each([
    ['closed_unmerged', /closed.*Create a New Launch for a separate attempt/i],
    ['merged', /merged pull request cannot be updated from CMA/i],
  ] as const)(
    'explains the terminal %s PR state without offering another write',
    async (state, reason) => {
      api.readLaunchDiff.mockResolvedValue({
        ...DIFF,
        pull_request: pullRequestWithState(state),
      });
      render(<ContributionSurface ownerId="test-owner" launchId={LAUNCH.id} navigate={vi.fn()} />);

      expect(await screen.findByText(reason)).toBeInTheDocument();
      expect(screen.queryByRole('button', { name: 'Open pull request' })).toBeNull();
      expect(screen.queryByRole('button', { name: 'Update pull request' })).toBeNull();
      expect(screen.queryByRole('button', { name: 'Refresh status' })).toBeNull();
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
    const current = { ...resource(), failure: 'CMA returned a provider failure.' };
    launchResource.use.mockReturnValue(current);
    render(<ContributionSurface ownerId="test-owner" launchId={LAUNCH.id} navigate={vi.fn()} />);

    expect(screen.getByRole('alert')).toHaveTextContent('Contribution data may be stale.');
    await userEvent.click(screen.getByRole('button', { name: 'Retry refresh' }));
    expect(current.refresh).toHaveBeenCalledOnce();
  });

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

    expect(api.reviewPullRequest).toHaveBeenCalledTimes(2);
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
