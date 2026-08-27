import { readFileSync } from 'node:fs';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type {
  DiffReview,
  Launch,
  PullRequestDraft,
  PullRequestRecord,
  TriggerPreview,
} from '@chronoai/cma-sdk';
import { useState } from 'react';
import { ApiError } from '../api/client';
import type { AppRoute, AppRouter, NavigableRoute } from '../routing/routes';
import { ContributionSurface } from './ContributionSurface';
import { LaunchSurface } from './LaunchSurface';
import { PublicTriggerPreview } from './PublicTriggerPreview';
import { TriggerReview } from './TriggerReview';

const api = vi.hoisted(() => ({
  confirmPullRequest: vi.fn(),
  createOrResumeLaunch: vi.fn(),
  listTriggerConnections: vi.fn(),
  preflightTriggerConnection: vi.fn(),
  publishLaunchBranch: vi.fn(),
  readLaunch: vi.fn(),
  readLaunchDiff: vi.fn(),
  readLaunchProgress: vi.fn(),
  readTriggerPreview: vi.fn(),
  requestLaunchDiff: vi.fn(),
  reviewPullRequest: vi.fn(),
  startLaunch: vi.fn(),
}));

vi.mock('../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../api/client')>()),
  ...api,
}));

const DRAFT = JSON.parse(
  readFileSync('../sdk/typescript/tests/fixtures/launch.json', 'utf8'),
) as Launch;
const PREVIEW = JSON.parse(
  readFileSync('../sdk/typescript/tests/fixtures/trigger_preview.json', 'utf8'),
) as TriggerPreview;
const REVIEWED = JSON.parse(
  readFileSync('../sdk/typescript/tests/fixtures/diff_review.json', 'utf8'),
) as DiffReview;
const PR_DRAFT = JSON.parse(
  readFileSync('../sdk/typescript/tests/fixtures/pull_request_draft.json', 'utf8'),
) as PullRequestDraft;

const SESSION_ID = 'sesn_trigger_journey';
const BRANCH_NAME = `cma/${DRAFT.id}`;
const FORK_REPOSITORY = {
  ...DRAFT.revision_content.upstream.repository,
  repository_id: 11,
  repository_node_id: 'R_contributor_fork',
  owner_id: 21,
  owner_node_id: 'U_contributor',
  canonical_owner: 'contributor',
};

function readyLaunch(): Launch {
  return {
    ...DRAFT,
    state: 'ready',
    revision_number: 10,
    steps: DRAFT.steps.map((step) => ({
      ...step,
      status: step.sequence <= 11 ? 'completed' : 'pending',
      completed_at: step.sequence <= 11 ? '2026-08-27T00:00:10Z' : null,
    })),
    fork: {
      repository: FORK_REPOSITORY,
      reused: false,
      source_revision: DRAFT.source_sha,
      canonical_url: 'https://github.com/contributor/project',
      ready_at: '2026-08-27T00:00:04Z',
    },
    branch: {
      name: BRANCH_NAME,
      expected_head: DRAFT.source_sha,
      current_head: DRAFT.source_sha,
      canonical_url: `https://github.com/contributor/project/tree/${BRANCH_NAME}`,
      created_at: '2026-08-27T00:00:05Z',
    },
    workspace: {
      latest_checkpoint: { id: '55555555-5555-4555-8555-555555555555' },
    },
    session: { session_id: SESSION_ID },
    reach: {
      service_id: DRAFT.connection?.service_id ?? 'service-fixture',
      state: 'active',
      policy_generation: 1,
      policy_fence: 1,
      runtime_audience: 'runtime-fixture',
      expires_at_epoch_seconds: 1_800_000_000,
      numeric_binding_observed_at: '2026-08-27T00:00:06Z',
      refreshed_at: '2026-08-27T00:00:06Z',
      tombstone_acknowledged: false,
      agent_key_revocation_confirmed: false,
    },
    delivery: {
      launch_id: DRAFT.id,
      session_id: SESSION_ID,
      revision_digest: DRAFT.revision_digest,
      origin_key: `cma-trigger:${DRAFT.id}:${DRAFT.revision_digest}:initial`,
      payload_hash: 'a'.repeat(64),
      state: 'completed',
      attempt: 1,
      recovery_generation: 0,
      dispatch_fence_sequence: 42,
      queued_at: '2026-08-27T00:00:06Z',
      updated_at: '2026-08-27T00:00:09Z',
    },
    contribution_checkpoint_version: 1,
    started_at: '2026-08-27T00:00:01Z',
    updated_at: '2026-08-27T00:00:10Z',
  };
}

function openPullRequest(): PullRequestRecord {
  return {
    ...PR_DRAFT,
    provider_id: 101,
    provider_node_id: 'PR_provider_node',
    number: 17,
    canonical_url: 'https://github.com/upstream-owner/project/pull/17',
    state: 'open',
    base_revision: PR_DRAFT.expected_base_revision,
    head_revision: PR_DRAFT.expected_head_revision,
    observed_title: PR_DRAFT.title,
    observed_body_digest: PR_DRAFT.body_digest,
    mergeability: 'computing',
    checks: 'pending',
    confirmation_snapshot_digest: PR_DRAFT.intent_digest,
    confirmed_at: '2026-08-27T00:00:15Z',
    created_at: '2026-08-27T00:00:15Z',
    updated_at: '2026-08-27T00:00:15Z',
    provider_created_at: '2026-08-27T00:00:15Z',
    provider_updated_at: '2026-08-27T00:00:15Z',
  };
}

function Journey() {
  const [signedIn, setSignedIn] = useState(false);
  const [route, setRoute] = useState<AppRoute>({
    kind: 'trigger',
    publicId: PREVIEW.public_id,
  });
  const navigate: AppRouter['navigate'] = (next: NavigableRoute) => setRoute(next);

  if (!signedIn) {
    return <PublicTriggerPreview publicId={PREVIEW.public_id} onSignIn={() => setSignedIn(true)} />;
  }
  switch (route.kind) {
    case 'trigger':
      return <TriggerReview ownerId="test-owner" publicId={route.publicId} navigate={navigate} />;
    case 'trigger_recovery':
      return (
        <TriggerReview ownerId="test-owner" recoveryLaunchId={route.launchId} navigate={navigate} />
      );
    case 'launch':
      return <LaunchSurface ownerId="test-owner" launchId={route.launchId} navigate={navigate} />;
    case 'contribute':
      return (
        <ContributionSurface ownerId="test-owner" launchId={route.launchId} navigate={navigate} />
      );
    case 'session':
      return <p>Session {route.sessionId}</p>;
    default:
      return <p>Unexpected route</p>;
  }
}

let currentLaunch: Launch;
let currentReview: DiffReview;

beforeEach(() => {
  vi.clearAllMocks();
  currentLaunch = DRAFT;
  currentReview = {
    ...REVIEWED,
    revision_number: 10,
    state: 'ready',
    artifact: null,
    contribution: null,
    pull_request: null,
  };

  api.readTriggerPreview.mockResolvedValue(PREVIEW);
  api.createOrResumeLaunch.mockResolvedValue(DRAFT);
  api.listTriggerConnections.mockResolvedValue([DRAFT.connection]);
  api.preflightTriggerConnection.mockResolvedValue(DRAFT.connection);
  api.startLaunch.mockImplementation(async () => {
    currentLaunch = readyLaunch();
    return currentLaunch;
  });
  api.readLaunchProgress.mockResolvedValue({
    launch_id: DRAFT.id,
    events: [],
    next_cursor: 0,
  });
  api.readLaunch.mockImplementation(async () => currentLaunch);
  api.readLaunchDiff.mockImplementation(async () => currentReview);
  api.requestLaunchDiff.mockImplementation(async () => {
    currentReview = {
      ...REVIEWED,
      revision_number: 11,
      state: 'contributing',
      contribution: null,
      pull_request: null,
    };
    currentLaunch = {
      ...currentLaunch,
      state: 'contributing',
      revision_number: 11,
      diff: currentReview.artifact,
    };
    return currentLaunch;
  });
  api.publishLaunchBranch.mockImplementation(async () => {
    currentReview = {
      ...REVIEWED,
      revision_number: 12,
      state: 'pr_pending',
      pull_request: null,
    };
    currentLaunch = {
      ...currentLaunch,
      state: 'pr_pending',
      revision_number: 12,
      diff: currentReview.artifact,
      contribution: currentReview.contribution,
    };
    return currentLaunch;
  });
  api.reviewPullRequest.mockResolvedValue(PR_DRAFT);
  api.confirmPullRequest.mockImplementation(async () => {
    const pullRequest = openPullRequest();
    currentReview = {
      ...currentReview,
      revision_number: 13,
      state: 'pr_open',
      pull_request: pullRequest,
    };
    currentLaunch = {
      ...currentLaunch,
      state: 'pr_open',
      revision_number: 13,
      pull_request: pullRequest,
    };
    return currentLaunch;
  });
});

describe('CMA sign-in return', () => {
  it.each([
    ['expired', 'Sign-in expired', 'Sign in again to continue.'],
    ['cancelled', 'Sign-in was cancelled', 'Nothing was started.'],
    ['failed', 'Sign-in failed', 'Sign in again to retry.'],
  ] as const)(
    'shows fixed %s copy while preserving the public preview and retry',
    async (authError, title, detail) => {
      const onSignIn = vi.fn();
      const user = userEvent.setup();
      render(
        <PublicTriggerPreview
          publicId={PREVIEW.public_id}
          authError={authError}
          onSignIn={onSignIn}
        />,
      );

      expect(
        await screen.findByRole('heading', { name: PREVIEW.public_summary }),
      ).toBeInTheDocument();
      const alert = screen.getByRole('alert');
      expect(alert).toHaveTextContent(title);
      expect(alert).toHaveTextContent(detail);
      expect(api.createOrResumeLaunch).not.toHaveBeenCalled();
      expect(api.startLaunch).not.toHaveBeenCalled();

      await user.click(screen.getByRole('button', { name: 'Sign in to review' }));
      expect(onSignIn).toHaveBeenCalledOnce();
    },
  );
});

describe('public Trigger route identity', () => {
  it('clears a failed preview before a different Trigger succeeds', async () => {
    const next = {
      ...PREVIEW,
      public_id: 'public-trigger-b',
      public_summary: 'Second Trigger task',
    };
    api.readTriggerPreview.mockRejectedValueOnce(new Error('preview A unavailable'));
    api.readTriggerPreview.mockResolvedValueOnce(next);
    const view = render(<PublicTriggerPreview publicId="public-trigger-a" onSignIn={vi.fn()} />);

    expect(await screen.findByRole('alert')).toHaveTextContent('preview A unavailable');
    const firstSignal = api.readTriggerPreview.mock.calls[0]?.[1] as AbortSignal;
    view.rerender(<PublicTriggerPreview publicId={next.public_id} onSignIn={vi.fn()} />);

    expect(firstSignal.aborted).toBe(true);
    expect(screen.queryByText('preview A unavailable')).toBeNull();
    expect(await screen.findByRole('heading', { name: next.public_summary })).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: next.public_summary })).toHaveFocus();
  });

  it('clears a successful preview before a different Trigger returns 404', async () => {
    api.readTriggerPreview.mockResolvedValueOnce(PREVIEW);
    api.readTriggerPreview.mockRejectedValueOnce(
      new ApiError(404, 'provider detail must not render', 'not_found'),
    );
    const view = render(<PublicTriggerPreview publicId={PREVIEW.public_id} onSignIn={vi.fn()} />);

    expect(
      await screen.findByRole('heading', { name: PREVIEW.public_summary }),
    ).toBeInTheDocument();
    const firstSignal = api.readTriggerPreview.mock.calls[0]?.[1] as AbortSignal;
    view.rerender(<PublicTriggerPreview publicId="public-trigger-missing" onSignIn={vi.fn()} />);

    expect(firstSignal.aborted).toBe(true);
    expect(screen.queryByRole('heading', { name: PREVIEW.public_summary })).toBeNull();
    expect(await screen.findByRole('heading', { name: 'Trigger not found' })).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: 'Trigger not found' })).toHaveFocus();
    expect(screen.queryByText('provider detail must not render')).toBeNull();
  });
});

describe('CMA Trigger contributor journey', () => {
  it('uses one explicit command at every write boundary from README preview to PR', async () => {
    const user = userEvent.setup();
    render(<Journey />);

    expect(
      await screen.findByRole('heading', { name: PREVIEW.public_summary }),
    ).toBeInTheDocument();
    expect(screen.queryByText(DRAFT.effective.rendered_prompt)).toBeNull();
    await user.click(screen.getByRole('button', { name: 'Sign in to review' }));

    expect(
      await screen.findByRole('heading', { name: PREVIEW.public_summary }),
    ).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Start Launch' }));
    await user.click(
      within(screen.getByRole('dialog', { name: 'Start this Launch?' })).getByRole('button', {
        name: 'Start Launch',
      }),
    );

    expect(
      await screen.findByRole('heading', { name: 'upstream-owner/project' }),
    ).toBeInTheDocument();
    expect(screen.getByRole('link', { name: /Open CMA session/ })).toHaveAttribute(
      'href',
      `/sessions/${SESSION_ID}`,
    );
    await user.click(screen.getByRole('link', { name: 'Review contribution' }));

    await user.click(await screen.findByRole('button', { name: 'Review diff' }));
    expect(await screen.findByText('README.md')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Publish branch' }));
    await user.click(
      within(screen.getByRole('dialog', { name: 'Publish this reviewed branch?' })).getByRole(
        'button',
        { name: 'Publish branch' },
      ),
    );

    await user.click(await screen.findByRole('button', { name: 'Open pull request' }));
    const proposal = await screen.findByRole('dialog', { name: 'Open pull request?' });
    await user.click(within(proposal).getByRole('button', { name: 'Open pull request' }));

    const pullRequest = await screen.findByRole('link', { name: /#17 CMA contribution/ });
    expect(pullRequest).toHaveAttribute(
      'href',
      'https://github.com/upstream-owner/project/pull/17',
    );
    const pullRequestSection = screen
      .getByRole('heading', { name: 'Pull request' })
      .closest('section');
    expect(pullRequestSection).not.toBeNull();
    expect(within(pullRequestSection as HTMLElement).getByText('open')).toBeInTheDocument();
    expect(api.createOrResumeLaunch).toHaveBeenCalledOnce();
    expect(api.startLaunch).toHaveBeenCalledOnce();
    expect(api.requestLaunchDiff).toHaveBeenCalledOnce();
    expect(api.publishLaunchBranch).toHaveBeenCalledOnce();
    expect(api.confirmPullRequest).toHaveBeenCalledOnce();
    expect(api.reviewPullRequest).toHaveBeenCalledTimes(2);

    for (const call of [
      api.createOrResumeLaunch.mock.calls[0],
      api.startLaunch.mock.calls[0],
      api.requestLaunchDiff.mock.calls[0],
      api.publishLaunchBranch.mock.calls[0],
      api.confirmPullRequest.mock.calls[0],
    ]) {
      expect(JSON.stringify(call)).toMatch(/cma-web-[0-9a-f-]+/i);
    }
  });
});
