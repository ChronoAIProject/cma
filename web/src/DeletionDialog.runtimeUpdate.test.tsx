import { act, fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError } from '@cma/client';
import { DeletionDialog } from './DeletionDialog';
import { restoreDeletion, type DeletionRequest } from './deletion';
import { agentClient } from './test/agent';
import {
  delayedRetirementFixture,
  parkedRuntimeUpdateFixture,
  sandboxFixture,
} from './test/hierarchy';
import type { CmaWebClient } from './api/client';

const initial: DeletionRequest = {
  action: { kind: 'delete_sandbox', sandboxId: 'sbx_1' },
  key: 'delete-parked-update',
  accepted: false,
};

afterEach(() => {
  localStorage.clear();
  vi.useRealTimers();
});

function show(client: CmaWebClient) {
  return render(
    <DeletionDialog
      client={client}
      viewerId="owner"
      initial={initial}
      restored={false}
      onChanged={vi.fn()}
      onClose={vi.fn()}
    />,
  );
}

it('confirms a freshly read parked update using the existing deletion command', async () => {
  const deleteSandbox = vi.fn(async () => undefined);
  const updateSandboxRuntime = vi.fn();
  render(
    <DeletionDialog
      client={agentClient({
        readSandbox: async () => parkedRuntimeUpdateFixture(),
        deleteSandbox,
        updateSandboxRuntime,
      })}
      viewerId="owner"
      initial={initial}
      restored={false}
      onChanged={vi.fn()}
      onClose={vi.fn()}
    />,
  );
  const actor = userEvent.setup();
  await actor.click(await screen.findByRole('checkbox'));
  const confirm = screen.getByRole('button', { name: 'Request deletion' });
  expect(confirm).toBeEnabled();
  expect(screen.getByText(/stop the paused runtime update/)).toHaveTextContent(
    'This cannot be undone.',
  );
  await actor.click(confirm);
  expect(deleteSandbox).toHaveBeenCalledExactlyOnceWith('sbx_1', { idempotencyKey: initial.key });
  expect(await screen.findByRole('dialog', { name: 'Deleting Sandbox' })).toBeInTheDocument();
  expect(screen.queryByRole('dialog', { name: 'Sandbox deleted' })).not.toBeInTheDocument();
  expect(updateSandboxRuntime).not.toHaveBeenCalled();
});

it.each(['updating', 'checkpoint', 'unavailable', 'foreign-id'] as const)(
  'withdraws confirmation when the next read reports %s',
  async (condition) => {
    vi.useFakeTimers();
    const parked = parkedRuntimeUpdateFixture();
    const deleteSandbox = vi.fn();
    const readSandbox = vi.fn(async () => parked);
    show(agentClient({ readSandbox, deleteSandbox }));
    await act(async () => {});
    fireEvent.click(screen.getByRole('checkbox'));
    expect(screen.getByRole('button', { name: 'Request deletion' })).toBeEnabled();
    if (condition === 'unavailable')
      readSandbox.mockRejectedValue(new ApiError(503, 'Unavailable'));
    else
      readSandbox.mockResolvedValue({
        ...parked,
        id: condition === 'foreign-id' ? 'sbx_other' : parked.id,
        lifecycle: {
          ...parked.lifecycle!,
          checkpoint:
            condition === 'checkpoint'
              ? { phase: 'capture', epoch: 1, startedAt: '2026-11-19T00:00:00Z' }
              : undefined,
        },
        runtimeUpdate: {
          ...parked.runtimeUpdate!,
          state: condition === 'updating' ? 'updating' : 'failed',
        },
      });
    await act(() => vi.advanceTimersByTimeAsync(1500));
    const confirm = screen.getByRole('button', { name: 'Request deletion' });
    expect(confirm).toBeDisabled();
    fireEvent.click(confirm);
    expect(deleteSandbox).not.toHaveBeenCalled();
    expect(screen.queryByRole('dialog', { name: 'Sandbox deleted' })).not.toBeInTheDocument();
  },
);

it.each(['retiring', 'retired'] as const)(
  'observes %s without issuing a second delete',
  async (status) => {
    vi.useFakeTimers();
    const readSandbox = vi.fn(async () => parkedRuntimeUpdateFixture());
    const deleteSandbox = vi.fn();
    show(agentClient({ readSandbox, deleteSandbox }));
    await act(async () => {});
    readSandbox.mockResolvedValue(sandboxFixture(status));
    await act(() => vi.advanceTimersByTimeAsync(1500));
    expect(
      screen.getByRole('dialog', {
        name: status === 'retiring' ? 'Deleting Sandbox' : 'Sandbox deleted',
      }),
    ).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Request deletion' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Check request' })).not.toBeInTheDocument();
    expect(deleteSandbox).not.toHaveBeenCalled();
    if (status === 'retiring') {
      expect(restoreDeletion('owner')).toEqual({ ...initial, accepted: true });
      expect(screen.queryByRole('button', { name: 'Done' })).not.toBeInTheDocument();
    }
  },
);

it.each([
  new ApiError(503, 'Service unavailable'),
  new ApiError(409, 'Sandbox changed'),
  new Error('Connection lost'),
])('keeps an uncertain parked-update deletion on its original key: %s', async (failure) => {
  const deleteSandbox = vi.fn().mockRejectedValueOnce(failure).mockResolvedValue(undefined);
  const updateSandboxRuntime = vi.fn();
  const readSandbox = vi.fn(async () => parkedRuntimeUpdateFixture());
  show(agentClient({ readSandbox, deleteSandbox, updateSandboxRuntime }));
  const actor = userEvent.setup();
  await actor.click(await screen.findByRole('checkbox'));
  await actor.click(screen.getByRole('button', { name: 'Request deletion' }));
  expect(await screen.findByRole('alert')).toHaveTextContent(failure.message);
  expect(restoreDeletion('owner')).toEqual(initial);
  expect(deleteSandbox).toHaveBeenCalledOnce();
  expect(screen.queryByRole('dialog', { name: 'Sandbox deleted' })).not.toBeInTheDocument();
  await actor.click(screen.getByRole('button', { name: 'Check request' }));
  expect(deleteSandbox.mock.calls).toEqual([
    ['sbx_1', { idempotencyKey: initial.key }],
    ['sbx_1', { idempotencyKey: initial.key }],
  ]);
  expect(updateSandboxRuntime).not.toHaveBeenCalled();
});

it('explains an overdue retirement until the exact Sandbox reports retired', async () => {
  vi.useFakeTimers();
  const readSandbox = vi.fn(async () => parkedRuntimeUpdateFixture());
  const deleteSandbox = vi.fn();
  show(agentClient({ readSandbox, deleteSandbox }));
  await act(async () => {});
  readSandbox.mockResolvedValue(delayedRetirementFixture());
  await act(() => vi.advanceTimersByTimeAsync(1500));
  expect(screen.getByRole('dialog', { name: 'Deleting Sandbox' })).toBeInTheDocument();
  expect(screen.getByText('Cleanup taking longer')).toBeVisible();
  expect(screen.getByText(/exceeded its expected deletion time/)).toBeVisible();
  expect(
    screen.getByText(
      'Check Environments for your current Sandbox capacity while deletion finishes.',
    ),
  ).toBeVisible();
  expect(restoreDeletion('owner')).toEqual({ ...initial, accepted: true });
  expect(screen.queryByRole('button', { name: 'Check request' })).not.toBeInTheDocument();
  readSandbox.mockResolvedValue(sandboxFixture('retired'));
  await act(() => vi.advanceTimersByTimeAsync(1500));
  expect(screen.getByRole('dialog', { name: 'Sandbox deleted' })).toBeInTheDocument();
  expect(screen.queryByText(/exceeded its expected deletion time/)).not.toBeInTheDocument();
  expect(deleteSandbox).not.toHaveBeenCalled();
});
