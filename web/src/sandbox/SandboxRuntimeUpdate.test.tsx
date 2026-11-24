import { act, fireEvent, render, screen } from '@testing-library/react';
import { ApiError } from '@cma/client';
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
  it.each([
    [true, 'Retry the runtime update. Recovery can use the latest runtime on your retained home.'],
    [
      true,
      'Retry the runtime update to continue with the requested runtime on your retained home.',
    ],
    [
      false,
      'Retry is unavailable. Contact the deployment operator for help recovering your retained home.',
    ],
  ] as const)(
    'shows the reported recovery path with retry eligibility %s',
    (canUpdate, guidance) => {
      const updateSandboxRuntime = vi.fn();
      const message = `The runtime update stopped at its current step. ${guidance}`;
      render(
        <SandboxRuntimeUpdate
          {...props()}
          sandboxStatus="rotating"
          client={agentClient({ updateSandboxRuntime })}
          update={{
            ...available,
            state: 'failed',
            canUpdate,
            failure: { code: 'terminal_validation', message },
          }}
        />,
      );
      expect(screen.getByRole('alert')).toHaveTextContent(message);
      expect(screen.getByRole('alert')).not.toHaveTextContent(/paused safely/i);
      const retry = screen.getByRole('button', { name: 'Retry runtime update' });
      if (canUpdate) expect(retry).toBeEnabled();
      else expect(retry).toBeDisabled();
      expect(updateSandboxRuntime).not.toHaveBeenCalled();
    },
  );

  it('does not advise retry when the client lacks the update method', () => {
    render(
      <SandboxRuntimeUpdate
        {...props()}
        sandboxStatus="rotating"
        update={{
          ...available,
          state: 'failed',
          failure: {
            code: 'terminal_validation',
            message:
              'The runtime update stopped at its current step. Retry the runtime update. Recovery can use the latest runtime on your retained home.',
          },
        }}
      />,
    );
    expect(screen.getByRole('alert')).toHaveTextContent('Retry is unavailable in this client.');
    expect(screen.getByRole('alert')).not.toHaveTextContent(
      /retry the runtime update|latest runtime/i,
    );
    expect(screen.getByRole('button', { name: 'Retry runtime update' })).toBeDisabled();
  });

  it('replaces older server guidance for an unavailable retry without asserting home retention', () => {
    const updateSandboxRuntime = vi.fn();
    render(
      <SandboxRuntimeUpdate
        {...props()}
        sandboxStatus="rotating"
        client={agentClient({ updateSandboxRuntime })}
        update={{
          ...available,
          state: 'failed',
          canUpdate: false,
          preservesHome: false,
          failure: {
            code: 'terminal_validation',
            message:
              'The runtime update paused safely. Your home and conversations are retained. Retry the update to continue.',
          },
        }}
      />,
    );
    expect(screen.getByRole('alert')).toHaveTextContent('stopped at its current step');
    expect(screen.getByRole('alert')).toHaveTextContent('Contact the deployment operator');
    expect(screen.getByRole('alert')).not.toHaveTextContent(/paused safely|retained|retry the/i);
    expect(screen.getByRole('button', { name: 'Retry runtime update' })).toBeDisabled();
    expect(updateSandboxRuntime).not.toHaveBeenCalled();
  });

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

  it.each([
    [available, 'Update runtime'],
    [
      { ...available, state: 'failed', targetVersion: 'registry.example/runtime@sha256:pinned' },
      'Retry runtime update',
    ],
  ] as const)(
    'retains the original target and key through navigation after %j',
    async (update, label) => {
      const updateSandboxRuntime = vi.fn().mockRejectedValue(new Error('Response lost'));
      const input = { ...props(), update, client: agentClient({ updateSandboxRuntime }) };
      const first = render(<SandboxRuntimeUpdate {...input} />);
      await act(async () => fireEvent.click(screen.getByRole('button', { name: label })));
      const initial = updateSandboxRuntime.mock.calls[0]!;
      first.unmount();
      render(
        <SandboxRuntimeUpdate
          {...input}
          update={{ ...update, latestVersion: 'registry.example/runtime@sha256:newer' }}
        />,
      );
      expect(screen.getByText('Requested runtime')).toBeVisible();
      await act(async () =>
        fireEvent.click(screen.getByRole('button', { name: 'Retry update request' })),
      );
      expect(updateSandboxRuntime.mock.calls[1]?.[1]).toBe(initial[1]);
      expect(updateSandboxRuntime.mock.calls[1]?.[2].idempotencyKey).toBe(
        initial[2].idempotencyKey,
      );
    },
  );

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
          canUpdate: false,
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
