import { readFileSync } from 'node:fs';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { Launch } from '@chronoai/cma-sdk';
import { TransportError } from '../api/client';
import { LaunchSurface } from './LaunchSurface';

const api = vi.hoisted(() => ({
  cancelLaunch: vi.fn(),
  createNewLaunch: vi.fn(),
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

function resource(launch: Launch) {
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
  launchResource.use.mockReturnValue(resource(FIXTURE));
});

describe('Launch command reconciliation', () => {
  it('retains one cancellation key and confirmation after an ambiguous response', async () => {
    api.cancelLaunch.mockRejectedValue(new TransportError('response lost'));
    const currentResource = resource(FIXTURE);
    launchResource.use.mockReturnValue(currentResource);
    render(<LaunchSurface launchId={FIXTURE.id} navigate={vi.fn()} />);
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
    expect(currentResource.accept).toHaveBeenCalledWith(cancelled);
    expect(screen.queryByRole('dialog', { name: 'Cancel this Launch?' })).toBeNull();
  });

  it('reconciles New Launch creation without issuing a new logical command', async () => {
    const completed = { ...FIXTURE, state: 'completed' as const };
    launchResource.use.mockReturnValue(resource(completed));
    api.createNewLaunch.mockRejectedValue(new TransportError('response lost'));
    const navigate = vi.fn();
    render(<LaunchSurface launchId={completed.id} navigate={navigate} />);
    const user = userEvent.setup();

    await user.click(screen.getByRole('button', { name: 'New Launch' }));
    const dialog = screen.getByRole('dialog', { name: 'Create a separate Launch?' });
    await user.click(within(dialog).getByRole('button', { name: 'Create New Launch' }));
    await waitFor(() => expect(api.createNewLaunch).toHaveBeenCalledTimes(2));

    const next = { ...FIXTURE, id: '55555555-5555-4555-8555-555555555555' };
    api.createNewLaunch.mockResolvedValue(next);
    await user.click(within(dialog).getByRole('button', { name: 'Reconcile New Launch' }));

    await waitFor(() => expect(api.createNewLaunch).toHaveBeenCalledTimes(3));
    const keys = api.createNewLaunch.mock.calls.map((call) => call[1].idempotencyKey);
    expect(new Set(keys)).toHaveLength(1);
    expect(navigate).toHaveBeenCalledWith({ kind: 'trigger', publicId: next.trigger_public_id });
  });
});
