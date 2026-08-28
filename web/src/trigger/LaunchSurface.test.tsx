import { readFileSync } from 'node:fs';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { DiffReview, Launch, PromptDelivery } from '@chronoai/cma-sdk';
import { TransportError } from '../api/client';
import { LaunchSurface } from './LaunchSurface';

const api = vi.hoisted(() => ({
  cancelLaunch: vi.fn(),
  createNewLaunch: vi.fn(),
  recoverInitialPrompt: vi.fn(),
}));

const launchResource = vi.hoisted(() => ({ use: vi.fn() }));

vi.mock('../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../api/client')>()),
  ...api,
}));

vi.mock('./useLaunchResource', () => ({
  useLaunchResource: launchResource.use,
}));

const FIXTURE = JSON.parse(
  readFileSync('../sdk/typescript/tests/fixtures/launch.json', 'utf8'),
) as Launch;
const RETAINED_REVIEW = JSON.parse(
  readFileSync('../sdk/typescript/tests/fixtures/diff_review.json', 'utf8'),
) as DiffReview;

const RECOVERY_DELIVERY: PromptDelivery = {
  launch_id: FIXTURE.id,
  session_id: 'session_fixture',
  revision_digest: FIXTURE.revision_digest,
  origin_key: `trigger:${FIXTURE.id}:initial-prompt`,
  payload_hash: 'b'.repeat(64),
  state: 'recovery_required',
  attempt: 1,
  recovery_generation: 0,
  dispatch_fence_sequence: 42,
  queued_at: '2026-08-26T12:00:00Z',
  updated_at: '2026-08-26T12:01:00Z',
};

const RECOVERY_LAUNCH: Launch = {
  ...FIXTURE,
  state: 'failed_retryable',
  revision_number: 12,
  blocked_reason: 'recovery_required',
  delivery: RECOVERY_DELIVERY,
};

function resource(launch: Launch) {
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
  launchResource.use.mockReturnValue(resource(FIXTURE));
});

describe('Launch command reconciliation', () => {
  it('keeps a historical Launch inspectable and cancellable without offering new work', async () => {
    if (FIXTURE.runtime_admission === null) throw new Error('fixture omitted runtime admission');
    const legacy: Launch = {
      ...RECOVERY_LAUNCH,
      runtime_admission: { ...FIXTURE.runtime_admission, kernel: 'codex' },
      session: { session_id: 'session_historical' },
      diff: RETAINED_REVIEW.artifact,
    };
    const navigate = vi.fn();
    launchResource.use.mockReturnValue(resource(legacy));
    render(<LaunchSurface ownerId="test-owner" launchId={legacy.id} navigate={navigate} />);

    expect(screen.getByText('Historical Launch is read-only')).toBeInTheDocument();
    expect(screen.getByText('Codex (historical, read-only)')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Refresh' })).toBeEnabled();
    expect(screen.getByRole('button', { name: 'New Launch' })).toBeEnabled();
    expect(screen.getByRole('button', { name: 'Cancel Launch' })).toBeEnabled();
    expect(screen.queryByRole('button', { name: 'Recover initial task' })).toBeNull();
    expect(screen.queryByRole('link', { name: 'Resume GitHub access' })).toBeNull();

    const session = screen.getByRole('link', { name: /Open CMA session/ });
    const contribution = screen.getByRole('link', { name: 'Inspect retained contribution' });
    await userEvent.click(session);
    expect(navigate).toHaveBeenCalledWith({ kind: 'session', sessionId: 'session_historical' });
    await userEvent.click(contribution);
    expect(navigate).toHaveBeenCalledWith({ kind: 'contribute', launchId: legacy.id });
    expect(api.recoverInitialPrompt).not.toHaveBeenCalled();
  });

  it('shows the display-only work label independently from the Launch branch', () => {
    const labeled: Launch = {
      ...FIXTURE,
      effective: { ...FIXTURE.effective, work_label: 'Documentation fix' },
    };
    launchResource.use.mockReturnValue(resource(labeled));
    render(<LaunchSurface ownerId="test-owner" launchId={labeled.id} navigate={vi.fn()} />);

    const label = screen.getByText('Work label').closest('div');
    const branch = screen.getByText('Launch branch').closest('div');
    expect(label).toHaveTextContent('Documentation fix');
    expect(branch).toHaveTextContent('Not created');
    expect(branch).not.toHaveTextContent('Documentation fix');
  });

  it('discloses an online refresh failure and retries the retained Launch read', async () => {
    const current = {
      ...resource(FIXTURE),
      failure: 'CMA returned a rate-limit response.',
      failureDisposition: 'retryable' as const,
    };
    launchResource.use.mockReturnValue(current);
    render(<LaunchSurface ownerId="test-owner" launchId={FIXTURE.id} navigate={vi.fn()} />);

    expect(screen.getByRole('alert')).toHaveTextContent('Launch data may be stale.');
    await userEvent.click(screen.getByRole('button', { name: 'Retry refresh' }));
    expect(current.refresh).toHaveBeenCalledOnce();
  });

  it('does not offer retry for a terminal Launch read failure', () => {
    launchResource.use.mockReturnValue({
      ...resource(FIXTURE),
      failure: 'This Launch does not exist.',
      failureDisposition: 'terminal',
    });
    render(<LaunchSurface ownerId="test-owner" launchId={FIXTURE.id} navigate={vi.fn()} />);

    expect(screen.getByRole('alert')).toHaveTextContent('This Launch does not exist.');
    expect(screen.queryByRole('button', { name: 'Retry refresh' })).toBeNull();
  });

  it('does not retry before the server-owned polling deadline', () => {
    const retryAt = Date.now() + 60_000;
    launchResource.use.mockReturnValue({
      ...resource(FIXTURE),
      failure: 'CMA returned a rate-limit response.',
      failureDisposition: 'retryable',
      nextRetryAt: retryAt,
    });
    render(<LaunchSurface ownerId="test-owner" launchId={FIXTURE.id} navigate={vi.fn()} />);

    expect(screen.getByText(/Automatic retry after/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Retry refresh' })).toBeDisabled();
  });

  it('routes an authentication-blocked Launch back to same-Launch recovery', async () => {
    const navigate = vi.fn();
    const blocked: Launch = {
      ...FIXTURE,
      state: 'auth_required',
      blocked_reason: 'owner_credential_unavailable',
      resume_state: 'preflight',
      started_at: '2026-08-26T12:00:00Z',
    };
    launchResource.use.mockReturnValue(resource(blocked));
    render(<LaunchSurface ownerId="test-owner" launchId={blocked.id} navigate={navigate} />);

    await userEvent.click(screen.getByRole('link', { name: 'Resume GitHub access' }));
    expect(navigate).toHaveBeenCalledWith({
      kind: 'trigger_recovery',
      launchId: blocked.id,
    });
  });

  it('renders a feature pause as a frozen CMA boundary without implying session failure', () => {
    const paused: Launch = {
      ...FIXTURE,
      state: 'prompt_pending',
      blocked_reason: 'feature_paused',
      steps: FIXTURE.steps.map((step) => ({
        ...step,
        status:
          step.key === 'initial_prompt'
            ? 'running'
            : step.sequence < 10
              ? 'completed'
              : step.status,
      })),
    };
    launchResource.use.mockReturnValue(resource(paused));

    render(<LaunchSurface ownerId="test-owner" launchId={paused.id} navigate={vi.fn()} />);

    const frozenRow = screen.getByText('Send initial task').closest('li');
    expect(frozenRow).toHaveAttribute('data-status', 'running');
    expect(frozenRow).toHaveAttribute('data-feature-paused', 'true');
    expect(frozenRow).toHaveTextContent('Paused by CMA');
    expect(screen.getAllByText('Paused by CMA')).toHaveLength(2);
    expect(screen.getByRole('status')).toHaveTextContent(
      'Any active CMA session remains available and was not interrupted.',
    );
    expect(screen.queryByText('This Launch remains resumable.')).toBeNull();
  });

  it('offers prompt recovery only for the exact retained recovery state', () => {
    launchResource.use.mockReturnValue(resource(RECOVERY_LAUNCH));
    const view = render(
      <LaunchSurface ownerId="test-owner" launchId={RECOVERY_LAUNCH.id} navigate={vi.fn()} />,
    );
    expect(screen.getByRole('button', { name: 'Recover initial task' })).toBeInTheDocument();

    const ineligible: readonly Launch[] = [
      { ...RECOVERY_LAUNCH, state: 'ready' as const },
      { ...RECOVERY_LAUNCH, blocked_reason: 'service_inactive' },
      {
        ...RECOVERY_LAUNCH,
        delivery: { ...RECOVERY_DELIVERY, state: 'failed' as const },
      },
      {
        ...RECOVERY_LAUNCH,
        delivery: { ...RECOVERY_DELIVERY, payload_hash: '' },
      },
    ];
    for (const launch of ineligible) {
      launchResource.use.mockReturnValue(resource(launch));
      view.rerender(<LaunchSurface ownerId="test-owner" launchId={launch.id} navigate={vi.fn()} />);
      expect(screen.queryByRole('button', { name: 'Recover initial task' })).toBeNull();
    }
  });

  it('requires acknowledgement and reconciles the frozen recovery authorization', async () => {
    api.recoverInitialPrompt.mockRejectedValue(new TransportError('response lost'));
    const initialResource = resource(RECOVERY_LAUNCH);
    launchResource.use.mockReturnValue(initialResource);
    const view = render(
      <LaunchSurface ownerId="test-owner" launchId={RECOVERY_LAUNCH.id} navigate={vi.fn()} />,
    );
    const user = userEvent.setup();

    const opener = screen.getByRole('button', { name: 'Recover initial task' });
    await user.click(opener);
    const dialog = screen.getByRole('dialog', { name: 'Authorize another initial task?' });
    expect(
      within(dialog).getByText(/CMA cannot determine whether the previous initial task started/i),
    ).toBeInTheDocument();
    const confirm = within(dialog).getByRole('button', {
      name: 'Authorize another dispatch',
    });
    expect(confirm).toBeDisabled();
    await user.click(
      within(dialog).getByRole('checkbox', {
        name: /I understand that the initial task may execute twice/i,
      }),
    );
    await user.click(confirm);

    await waitFor(() => expect(api.recoverInitialPrompt).toHaveBeenCalledTimes(2));
    const first = api.recoverInitialPrompt.mock.calls[0];
    const second = api.recoverInitialPrompt.mock.calls[1];
    expect(first?.slice(0, 3)).toEqual([
      RECOVERY_LAUNCH.id,
      12,
      RECOVERY_LAUNCH.delivery?.payload_hash,
    ]);
    expect(first?.[3].idempotencyKey).toBe(second?.[3].idempotencyKey);
    expect(await within(dialog).findByText(/recovery outcome is unknown/i)).toBeInTheDocument();
    expect(within(dialog).getByRole('button', { name: 'Close' })).toBeDisabled();
    expect(within(dialog).getByRole('button', { name: 'Cancel' })).toBeDisabled();
    await user.keyboard('{Escape}');
    expect(screen.getByRole('dialog', { name: 'Authorize another initial task?' })).toBe(dialog);

    const refreshedLaunch: Launch = {
      ...RECOVERY_LAUNCH,
      revision_number: 99,
      delivery: { ...RECOVERY_DELIVERY, payload_hash: 'c'.repeat(64) },
    };
    const refreshedResource = resource(refreshedLaunch);
    launchResource.use.mockReturnValue(refreshedResource);
    view.rerender(
      <LaunchSurface ownerId="test-owner" launchId={refreshedLaunch.id} navigate={vi.fn()} />,
    );
    api.recoverInitialPrompt.mockResolvedValue({
      ...refreshedLaunch,
      state: 'ready',
      blocked_reason: null,
    });

    await user.click(
      within(dialog).getByRole('checkbox', {
        name: /I understand that the initial task may execute twice/i,
      }),
    );
    await user.click(within(dialog).getByRole('button', { name: 'Reconcile recovery' }));

    await waitFor(() => expect(api.recoverInitialPrompt).toHaveBeenCalledTimes(3));
    const third = api.recoverInitialPrompt.mock.calls[2];
    expect(third?.slice(0, 3)).toEqual(first?.slice(0, 3));
    expect(third?.[3].idempotencyKey).toBe(first?.[3].idempotencyKey);
    await waitFor(() => expect(refreshedResource.accept).toHaveBeenCalledOnce());
    expect(screen.queryByRole('dialog', { name: 'Authorize another initial task?' })).toBeNull();
    await waitFor(() => expect(opener).toHaveFocus());
  });

  it('renders a hostile finite recovery failure as text and keeps the dialog available', async () => {
    const hostile = '<img src=x onerror=alert(1)> recovery refused';
    api.recoverInitialPrompt.mockRejectedValue(new Error(hostile));
    launchResource.use.mockReturnValue(resource(RECOVERY_LAUNCH));
    render(<LaunchSurface ownerId="test-owner" launchId={RECOVERY_LAUNCH.id} navigate={vi.fn()} />);
    const user = userEvent.setup();

    await user.click(screen.getByRole('button', { name: 'Recover initial task' }));
    const dialog = screen.getByRole('dialog', { name: 'Authorize another initial task?' });
    await user.click(within(dialog).getByRole('checkbox'));
    await user.click(within(dialog).getByRole('button', { name: 'Authorize another dispatch' }));

    expect(await within(dialog).findByText(hostile)).toBeInTheDocument();
    expect(dialog.querySelector('img')).toBeNull();
    expect(within(dialog).getByRole('button', { name: 'Close' })).toBeEnabled();
  });

  it('keeps Launch and acknowledgement targets at least 44px on narrow screens', () => {
    const triggerCss = readFileSync('src/trigger/Trigger.css', 'utf8');
    const riskCss = readFileSync('src/ui/RiskConfirmation.css', 'utf8');
    expect(triggerCss).toMatch(
      /@media \(max-width: 29\.999rem\)[\s\S]*\.trigger-workflow__actions \.button\s*\{[^}]*min-height: 44px/,
    );
    expect(riskCss).toMatch(
      /@media \(max-width: 29\.999rem\)[\s\S]*\.risk__acknowledgement\s*\{[^}]*min-height: 44px/,
    );
  });

  it('retains one cancellation key and confirmation after an ambiguous response', async () => {
    api.cancelLaunch.mockRejectedValue(new TransportError('response lost'));
    const currentResource = resource(FIXTURE);
    launchResource.use.mockReturnValue(currentResource);
    render(<LaunchSurface ownerId="test-owner" launchId={FIXTURE.id} navigate={vi.fn()} />);
    const user = userEvent.setup();

    await user.click(screen.getByRole('button', { name: 'Cancel Launch' }));
    const dialog = screen.getByRole('dialog', { name: 'Cancel this Launch?' });
    await user.click(within(dialog).getByRole('button', { name: 'Cancel Launch' }));

    await waitFor(() => expect(api.cancelLaunch).toHaveBeenCalledTimes(2));
    expect(api.cancelLaunch.mock.calls[0]?.[3].idempotencyKey).toBe(
      api.cancelLaunch.mock.calls[1]?.[3].idempotencyKey,
    );
    expect(await screen.findByText(/cancellation outcome is unknown/i)).toBeInTheDocument();
    expect(within(dialog).getByRole('button', { name: 'Reconcile cancellation' })).toBeEnabled();

    const cancelled = { ...FIXTURE, state: 'cancelled' as const, revision_number: 1 };
    api.cancelLaunch.mockResolvedValue(cancelled);
    await user.click(within(dialog).getByRole('button', { name: 'Reconcile cancellation' }));

    await waitFor(() => expect(api.cancelLaunch).toHaveBeenCalledTimes(3));
    expect(api.cancelLaunch.mock.calls[2]?.[3].idempotencyKey).toBe(
      api.cancelLaunch.mock.calls[0]?.[3].idempotencyKey,
    );
    await waitFor(() => expect(currentResource.accept).toHaveBeenCalledWith(cancelled));
    expect(screen.queryByRole('dialog', { name: 'Cancel this Launch?' })).toBeNull();
  });

  it('reconciles New Launch creation without issuing a new logical command', async () => {
    launchResource.use.mockReturnValue(resource(FIXTURE));
    api.createNewLaunch.mockRejectedValue(new TransportError('response lost'));
    const navigate = vi.fn();
    render(<LaunchSurface ownerId="test-owner" launchId={FIXTURE.id} navigate={navigate} />);
    const user = userEvent.setup();

    expect(screen.getByRole('button', { name: 'Cancel Launch' })).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'New Launch' }));
    const dialog = screen.getByRole('dialog', { name: 'Create a separate Launch?' });
    await user.click(within(dialog).getByRole('button', { name: 'Create New Launch' }));
    await waitFor(() => expect(api.createNewLaunch).toHaveBeenCalledTimes(2));

    const next = { ...FIXTURE, id: '55555555-5555-4555-8555-555555555555' };
    api.createNewLaunch.mockResolvedValue(next);
    await user.click(await within(dialog).findByRole('button', { name: 'Reconcile New Launch' }));

    await waitFor(() => expect(api.createNewLaunch).toHaveBeenCalledTimes(3));
    const keys = api.createNewLaunch.mock.calls.map((call) => call[1].idempotencyKey);
    expect(new Set(keys)).toHaveLength(1);
    await waitFor(() =>
      expect(navigate).toHaveBeenCalledWith({ kind: 'trigger', publicId: next.trigger_public_id }),
    );
  });
});
