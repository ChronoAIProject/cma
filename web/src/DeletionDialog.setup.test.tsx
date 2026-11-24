import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { DeletionDialog } from './DeletionDialog';
import { agentClient } from './test/agent';
import { sandboxFixture } from './test/hierarchy';

afterEach(() => {
  vi.useRealTimers();
  localStorage.clear();
});

it.each(['provisioning', 'installing', 'active'] as const)(
  'confirms the whole %s environment, and cancels without any write',
  async (status) => {
    const deleteSandbox = vi.fn(),
      deleteAgent = vi.fn(),
      stopAgent = vi.fn(),
      close = vi.fn();
    render(
      <DeletionDialog
        viewerId="owner"
        client={agentClient({
          readSandbox: async () => sandboxFixture(status),
          deleteSandbox,
          deleteAgent,
          stopAgent,
        })}
        initial={{
          action: { kind: 'delete_sandbox', sandboxId: 'sbx_1' },
          key: 'setup-delete',
          accepted: false,
        }}
        restored={false}
        onChanged={vi.fn()}
        onClose={close}
      />,
    );
    await act(async () => {});
    expect(screen.getByRole('dialog', { name: 'Delete Sandbox' })).toHaveTextContent(
      'This deletes the whole Sandbox environment, including all Agents, Workspaces, and files inside it.',
    );
    expect(screen.queryByText(/cancel setup/i)).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('checkbox'));
    expect(screen.getByRole('button', { name: 'Request deletion' })).toBeEnabled();
    fireEvent.click(screen.getByRole('button', { name: 'Keep environment' }));
    expect(close).toHaveBeenCalledOnce();
    expect(deleteSandbox).not.toHaveBeenCalled();
    expect(deleteAgent).not.toHaveBeenCalled();
    expect(stopAgent).not.toHaveBeenCalled();
  },
);

it.each(['checkpoint', 'replacement', 'retiring'] as const)(
  'rechecks a setup confirmation when the parent changes to %s',
  async (change) => {
    vi.useFakeTimers();
    let sandbox = sandboxFixture('installing');
    const deleteSandbox = vi.fn();
    render(
      <DeletionDialog
        viewerId="owner"
        client={agentClient({ readSandbox: async () => sandbox, deleteSandbox })}
        initial={{
          action: { kind: 'delete_sandbox', sandboxId: 'sbx_1' },
          key: 'setup-race',
          accepted: false,
        }}
        restored={false}
        onChanged={vi.fn()}
        onClose={vi.fn()}
      />,
    );
    await act(async () => {});
    fireEvent.click(screen.getByRole('checkbox'));
    expect(screen.getByRole('button', { name: 'Request deletion' })).toBeEnabled();
    sandbox = {
      ...sandbox,
      status: change === 'retiring' ? 'retiring' : 'installing',
      lifecycle: {
        ...sandbox.lifecycle!,
        checkpoint:
          change === 'checkpoint'
            ? { phase: 'capture', epoch: 1, startedAt: '2026-11-20T00:00:00Z' }
            : undefined,
        operation:
          change === 'replacement'
            ? { purpose: 'image_rotation', phase: 'replace', startedAt: '2026-11-20T00:00:00Z' }
            : undefined,
      },
    };
    await act(() => vi.advanceTimersByTimeAsync(1500));
    if (change === 'retiring') {
      expect(screen.getByRole('dialog', { name: 'Deleting Sandbox' })).toHaveTextContent(
        'Delete request accepted',
      );
      expect(screen.queryByRole('button', { name: 'Check request' })).not.toBeInTheDocument();
    } else {
      expect(screen.getByRole('button', { name: 'Request deletion' })).toBeDisabled();
      expect(screen.getByRole('dialog')).toHaveTextContent(
        'Wait for the current Sandbox operation to finish.',
      );
    }
    expect(deleteSandbox).not.toHaveBeenCalled();
  },
);
