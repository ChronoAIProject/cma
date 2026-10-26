import { act, fireEvent, render, screen } from '@testing-library/react';
import { ApiError } from '@chronoai/cma-sdk';
import { describe, expect, it, vi } from 'vitest';
import type { SandboxRuntimeUpdate as Update } from '../api/types';
import { agentClient } from '../test/agent';
import { SandboxRuntimeUpdate } from './SandboxRuntimeUpdate';

const icon = () => document.querySelector('.sandbox-recovery__icon .statedot');

const available: Update = {
  currentVersion: 'registry.example/runtime@sha256:old',
  latestVersion: 'registry.example/runtime@sha256:new',
  updateAvailable: true,
  state: 'available',
  canUpdate: true,
  schedule: { timezone: 'Asia/Singapore', localTime: '02:00' },
  preservesHome: true,
};
const props = () => ({
  client: agentClient(),
  sandboxId: 'sbx-runtime',
  ownerId: 'owner',
  revision: 5,
  update: available,
  stale: false,
  busy: false,
  onAccepted: vi.fn(),
});

describe('owner runtime update control', () => {
  it('retries a failed accepted target without switching to a newer deployment', async () => {
    const updateSandboxRuntime = vi.fn().mockResolvedValue({
      runtimeUpdate: { ...available, state: 'queued', canUpdate: false },
      revision: 6,
    });
    render(
      <SandboxRuntimeUpdate
        {...props()}
        client={agentClient({ updateSandboxRuntime })}
        update={{
          ...available,
          state: 'failed',
          targetVersion: available.latestVersion,
          latestVersion: 'registry.example/runtime@sha256:newer',
        }}
      />,
    );
    await act(async () =>
      fireEvent.click(screen.getByRole('button', { name: 'Retry runtime update' })),
    );
    expect(updateSandboxRuntime).toHaveBeenCalledWith(
      'sbx-runtime',
      available.latestVersion,
      expect.anything(),
    );
  });
  it('does not admit an update on navigation and submits the exact displayed target on click', async () => {
    const receipt = {
      runtimeUpdate: { ...available, state: 'queued' as const, canUpdate: false },
      revision: 6,
    };
    const updateSandboxRuntime = vi.fn().mockResolvedValue(receipt);
    const input = { ...props(), client: agentClient({ updateSandboxRuntime }) };
    render(<SandboxRuntimeUpdate {...input} />);
    expect(updateSandboxRuntime).not.toHaveBeenCalled();
    expect(screen.getByText(available.currentVersion!)).toBeVisible();
    expect(screen.getByText(/Daily at 02:00 SGT/)).toBeVisible();
    await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Update runtime' })));
    expect(updateSandboxRuntime).toHaveBeenCalledOnce();
    expect(updateSandboxRuntime).toHaveBeenCalledWith(
      'sbx-runtime',
      available.latestVersion,
      expect.objectContaining({ idempotencyKey: expect.any(String) }),
    );
    expect(screen.getByRole('status')).toHaveTextContent('Runtime update queued');
    expect(screen.getByText(/Waiting for current Agent turns, work and approvals/)).toBeVisible();
    expect(screen.getByRole('button', { name: 'Update queued' })).toBeDisabled();
    expect(input.onAccepted).toHaveBeenCalledWith(receipt);
  });

  it('retains exact uncertain target and key through navigation even when latest changes', async () => {
    const updateSandboxRuntime = vi.fn().mockRejectedValue(new Error('Response lost'));
    const input = { ...props(), client: agentClient({ updateSandboxRuntime }) };
    const first = render(<SandboxRuntimeUpdate {...input} />);
    await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Update runtime' })));
    const initial = updateSandboxRuntime.mock.calls[0]!;
    first.unmount();
    render(
      <SandboxRuntimeUpdate
        {...input}
        update={{ ...available, latestVersion: 'registry.example/runtime@sha256:newer' }}
      />,
    );
    expect(screen.getByText('Requested runtime')).toBeVisible();
    await act(async () =>
      fireEvent.click(screen.getByRole('button', { name: 'Retry update request' })),
    );
    expect(updateSandboxRuntime.mock.calls[1]?.[1]).toBe(initial[1]);
    expect(updateSandboxRuntime.mock.calls[1]?.[2].idempotencyKey).toBe(initial[2].idempotencyKey);
  });

  it('does not reuse another owner’s uncertain command', async () => {
    const updateSandboxRuntime = vi.fn().mockRejectedValue(new Error('Response lost'));
    const input = { ...props(), client: agentClient({ updateSandboxRuntime }) };
    const view = render(<SandboxRuntimeUpdate {...input} />);
    await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Update runtime' })));
    view.rerender(<SandboxRuntimeUpdate {...input} ownerId="another-owner" />);
    expect(screen.getByRole('button', { name: 'Update runtime' })).toBeEnabled();
    expect(screen.queryByRole('button', { name: 'Retry update request' })).not.toBeInTheDocument();
  });

  it('allows an explicit new target after confirmed refusal and prevents stale or busy writes', async () => {
    const updateSandboxRuntime = vi.fn().mockRejectedValue(new ApiError(409, 'Target changed'));
    const input = { ...props(), client: agentClient({ updateSandboxRuntime }) };
    const view = render(<SandboxRuntimeUpdate {...input} stale />);
    expect(screen.getByRole('button', { name: 'Update runtime' })).toBeDisabled();
    view.rerender(<SandboxRuntimeUpdate {...input} busy />);
    expect(screen.getByRole('button', { name: 'Update runtime' })).toBeDisabled();
    view.rerender(<SandboxRuntimeUpdate {...input} />);
    await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Update runtime' })));
    expect(screen.getByRole('alert')).toHaveTextContent(
      'available runtime or Sandbox status changed',
    );
    view.rerender(
      <SandboxRuntimeUpdate
        {...input}
        update={{ ...available, latestVersion: 'registry.example/runtime@sha256:newer' }}
      />,
    );
    await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Update runtime' })));
    expect(updateSandboxRuntime.mock.calls[1]?.[1]).toBe('registry.example/runtime@sha256:newer');
    expect(updateSandboxRuntime.mock.calls[1]?.[2].idempotencyKey).not.toBe(
      updateSandboxRuntime.mock.calls[0]?.[2].idempotencyKey,
    );
  });

  it('uses confirmed states for animation and does not claim home preservation without proof', () => {
    const input = props();
    const view = render(
      <SandboxRuntimeUpdate
        {...input}
        update={{ ...available, state: 'updating', canUpdate: false, preservesHome: false }}
      />,
    );
    expect(screen.getByRole('status')).toHaveTextContent('Updating your runtime');
    expect(icon()).toHaveAttribute('data-state', 'ongoing');
    expect(screen.queryByText(/existing home disk/)).not.toBeInTheDocument();
    view.rerender(
      <SandboxRuntimeUpdate
        {...input}
        update={{
          ...available,
          currentVersion: null,
          state: 'failed',
          failure: { code: 'restore_failed', message: 'Restore needs attention.' },
        }}
      />,
    );
    expect(screen.getByText('Not reported')).toBeVisible();
    expect(screen.getByRole('alert')).toHaveTextContent('Restore needs attention.');
    expect(icon()).toHaveAttribute('data-state', 'error');
    view.rerender(
      <SandboxRuntimeUpdate
        {...input}
        update={{
          ...available,
          state: 'up_to_date',
          updateAvailable: false,
          completedAt: '2026-10-19T02:00:00+08:00',
        }}
      />,
    );
    expect(screen.getByRole('status')).toHaveTextContent('Runtime updated');
    expect(icon()).toHaveAttribute('data-state', 'done');
    expect(screen.queryByRole('button')).not.toBeInTheDocument();
  });
});
