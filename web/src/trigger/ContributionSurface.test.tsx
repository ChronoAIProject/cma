import { readFileSync } from 'node:fs';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { DiffReview, Launch, PullRequestDraft } from '@chronoai/cma-sdk';
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

function readyLaunch(): Launch {
  return {
    ...LAUNCH,
    state: 'ready',
    session: {
      session_id: 'sesn_trigger',
      environment_profile_id: 'env_trigger',
      runtime_generation: 1,
      created_at: LAUNCH.created_at,
    },
  };
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
  it('replays diff capture with one key and no second logical command', async () => {
    const currentResource = resource();
    launchResource.use.mockReturnValue(currentResource);
    api.requestLaunchDiff.mockRejectedValue(new TransportError('response lost'));
    render(<ContributionSurface launchId={LAUNCH.id} navigate={vi.fn()} />);
    const user = userEvent.setup();

    await user.click(await screen.findByRole('button', { name: 'Review diff' }));
    await waitFor(() => expect(api.requestLaunchDiff).toHaveBeenCalledTimes(2));
    expect(api.requestLaunchDiff.mock.calls[0]?.[3].idempotencyKey).toBe(
      api.requestLaunchDiff.mock.calls[1]?.[3].idempotencyKey,
    );

    api.requestLaunchDiff.mockResolvedValue({ ...readyLaunch(), revision_number: 1 });
    await user.click(screen.getByRole('button', { name: 'Reconcile diff capture' }));
    await waitFor(() => expect(api.requestLaunchDiff).toHaveBeenCalledTimes(3));
    expect(
      new Set(api.requestLaunchDiff.mock.calls.map((call) => call[3].idempotencyKey)).size,
    ).toBe(1);
    expect(currentResource.accept).toHaveBeenCalledOnce();
  });

  it('keeps the reviewed artifact confirmation while branch publication is ambiguous', async () => {
    api.readLaunchDiff.mockResolvedValue({ ...DIFF, contribution: null });
    api.publishLaunchBranch.mockRejectedValue(new TransportError('response lost'));
    render(<ContributionSurface launchId={LAUNCH.id} navigate={vi.fn()} />);
    const user = userEvent.setup();

    await user.click(await screen.findByRole('button', { name: 'Publish branch' }));
    const dialog = screen.getByRole('dialog', { name: 'Publish this reviewed branch?' });
    await user.click(within(dialog).getByRole('button', { name: 'Publish branch' }));
    await waitFor(() => expect(api.publishLaunchBranch).toHaveBeenCalledTimes(2));
    expect(
      within(dialog).getByRole('button', { name: 'Reconcile branch publication' }),
    ).toBeEnabled();

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
    render(<ContributionSurface launchId={LAUNCH.id} navigate={vi.fn()} />);
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
});
