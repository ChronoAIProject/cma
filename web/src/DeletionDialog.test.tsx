import { act, fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError } from '@chronoai/cma-sdk';
import { DeletionDialog } from './DeletionDialog';
import {
  deletionStorageKey,
  observeDeletion,
  restoreDeletion,
  type DeletionRequest,
} from './deletion';
import { AGENT_DETAIL, agentClient } from './test/agent';
import { sandboxFixture, workspaceFixture } from './test/hierarchy';
import type { WorkspaceDetail } from './api/types';

const initial: DeletionRequest = {
  action: { kind: 'delete_sandbox', sandboxId: 'sbx_1' },
  key: 'delete-atlas',
  accepted: false,
};
afterEach(() => {
  vi.useRealTimers();
  localStorage.clear();
});

it('keeps the dialog open after acceptance, reports recorded steps, and waits for retirement', async () => {
  vi.useFakeTimers();
  let sandbox = sandboxFixture();
  const deleted = vi.fn(async () => {
    sandbox = sandboxFixture('retiring');
  });
  const changed = vi.fn();
  const close = vi.fn();
  render(
    <DeletionDialog
      viewerId="owner"
      client={agentClient({ readSandbox: async () => sandbox, deleteSandbox: deleted })}
      initial={initial}
      restored={false}
      onChanged={changed}
      onClose={close}
    />,
  );
  await act(async () => {});
  expect(screen.getByRole('button', { name: 'Delete Sandbox' })).toBeDisabled();
  fireEvent.click(screen.getByRole('checkbox'));
  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: 'Delete Sandbox' }));
  });
  expect(deleted).toHaveBeenCalledExactlyOnceWith('sbx_1', { idempotencyKey: initial.key });
  expect(screen.getByRole('dialog', { name: 'Deleting Sandbox' })).toBeInTheDocument();
  expect(screen.getByRole('status')).toHaveTextContent('Delete request accepted');
  expect(restoreDeletion('owner')?.accepted).toBe(true);
  expect(close).not.toHaveBeenCalled();
  await act(() => vi.advanceTimersByTimeAsync(4500));
  expect(screen.queryByRole('button', { name: 'Done' })).not.toBeInTheDocument();
  sandbox = sandboxFixture('retired');
  await act(() => vi.advanceTimersByTimeAsync(1500));
  expect(screen.getByRole('dialog', { name: 'Sandbox deleted' })).toBeInTheDocument();
  expect(restoreDeletion('owner')).toBeUndefined();
  expect(deleted).toHaveBeenCalledTimes(1);
  fireEvent.click(screen.getByRole('button', { name: 'Done' }));
  expect(close).toHaveBeenCalledTimes(1);
});

it('restores an accepted Workspace deletion using reads only, and waits for Removed', async () => {
  vi.useFakeTimers();
  const request: DeletionRequest = {
    action: { kind: 'remove_workspace', workspaceId: 'wsp_1' },
    key: 'workspace-delete',
    accepted: true,
  };
  localStorage.setItem(deletionStorageKey('owner'), JSON.stringify(request));
  let workspace: WorkspaceDetail = { ...workspaceFixture('stopped'), status: 'removing' };
  const removeWorkspace = vi.fn();
  const client = agentClient({ removeWorkspace, readWorkspace: async () => workspace });
  const view = render(
    <DeletionDialog
      viewerId="owner"
      client={client}
      initial={restoreDeletion('owner')!}
      restored
      onChanged={vi.fn()}
      onClose={vi.fn()}
    />,
  );
  await act(async () => {});
  expect(screen.getByRole('status')).toHaveTextContent('accepted');
  expect(removeWorkspace).not.toHaveBeenCalled();
  view.unmount();
  expect(restoreDeletion('owner')?.accepted).toBe(true);
  workspace = { ...workspace, status: 'removed' };
  render(
    <DeletionDialog
      viewerId="owner"
      client={client}
      initial={restoreDeletion('owner')!}
      restored
      onChanged={vi.fn()}
      onClose={vi.fn()}
    />,
  );
  await act(async () => {});
  expect(screen.getByRole('dialog', { name: 'Workspace deleted' })).toBeInTheDocument();
  expect(removeWorkspace).not.toHaveBeenCalled();
});

it('retains the same command key after an uncertain response and never automatically resubmits', async () => {
  const actor = userEvent.setup();
  const deleteSandbox = vi
    .fn()
    .mockRejectedValueOnce(new Error('Connection lost'))
    .mockResolvedValue(undefined);
  render(
    <DeletionDialog
      viewerId="owner"
      client={agentClient({ deleteSandbox })}
      initial={initial}
      restored
      onChanged={vi.fn()}
      onClose={vi.fn()}
    />,
  );
  await actor.click(screen.getByRole('button', { name: 'Check request' }));
  expect(await screen.findByText('Connection lost')).toBeInTheDocument();
  await actor.click(screen.getByRole('button', { name: 'Check request' }));
  expect(deleteSandbox.mock.calls.map((call) => call[1].idempotencyKey)).toEqual([
    initial.key,
    initial.key,
  ]);
});

it('does not declare success on read failures or a missing resource', async () => {
  const client = agentClient({
    readSandbox: async () => {
      throw new ApiError(404, 'Not found', 'not_found');
    },
  });
  render(
    <DeletionDialog
      viewerId="owner"
      client={client}
      initial={{ ...initial, accepted: true }}
      restored
      onChanged={vi.fn()}
      onClose={vi.fn()}
    />,
  );
  expect(await screen.findByRole('alert')).toHaveTextContent('could not be verified');
  expect(screen.queryByRole('button', { name: 'Done' })).not.toBeInTheDocument();
});

it('requires both Agent deletion and Worktree removal, not just a stopped Agent', async () => {
  const action = { kind: 'delete_agent', agentId: 'agt_1' } as const;
  for (const [status, worktree, complete] of [
    ['stopped', 'ready', false],
    ['deleted', 'removing', false],
    ['deleted', 'removed', true],
  ] as const) {
    const client = agentClient({
      readAgent: async () => ({
        ...AGENT_DETAIL,
        status,
        worktree: { ...AGENT_DETAIL.worktree, status: worktree },
      }),
    });
    expect(
      (await observeDeletion(client, action, new AbortController().signal, true)).complete,
    ).toBe(complete);
  }
});

it('names the stop prerequisite before Workspace deletion and rejects corrupt browser recovery data', async () => {
  const observation = await observeDeletion(
    agentClient(),
    { kind: 'remove_workspace', workspaceId: 'wsp_1' },
    new AbortController().signal,
    false,
  );
  expect(observation.blocked).toContain('Stop the Agents');
  localStorage.setItem(deletionStorageKey('owner'), '{broken');
  expect(restoreDeletion('owner')).toBeUndefined();
  expect(restoreDeletion('another-owner')).toBeUndefined();
});
