import { act, fireEvent, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError } from '@cma/client';
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

it('loads completed Agent deletion activity even before request acceptance is confirmed', async () => {
  const deleteAgent = vi.fn();
  const readAgentEvents = vi.fn(async () => ({
    items: [{ seq: 1, appendedAt: '2026-09-09T00:00:00Z', kind: { type: 'deleted' } }],
  }));
  const client = agentClient({
    readAgent: async () => ({
      ...AGENT_DETAIL,
      status: 'deleted',
      worktree: { ...AGENT_DETAIL.worktree, status: 'removed' },
    }),
    readAgentEvents,
    deleteAgent,
  });
  render(
    <DeletionDialog
      viewerId="owner"
      client={client}
      initial={{
        action: { kind: 'delete_agent', agentId: AGENT_DETAIL.id },
        key: 'uncertain-agent-delete',
        accepted: false,
      }}
      restored
      onChanged={vi.fn()}
      onClose={vi.fn()}
    />,
  );
  expect(await screen.findByRole('dialog', { name: 'Agent deleted' })).toBeInTheDocument();
  expect(screen.getByText(/Its Sandbox environment is retained and still counts/)).toBeVisible();
  expect(screen.getByRole('link', { name: 'Review parent environment' })).toHaveAttribute(
    'href',
    `/sandboxes/${AGENT_DETAIL.sandbox.id}`,
  );
  expect(screen.getByRole('region', { name: 'Recorded activity' })).not.toBeVisible();
  fireEvent.click(screen.getByText('Cleanup progress'));
  expect(readAgentEvents).toHaveBeenCalledTimes(1);
  expect(
    within(screen.getByRole('region', { name: 'Recorded activity' })).getByRole('listitem'),
  ).toHaveTextContent('Agent deleted');
  expect(deleteAgent).not.toHaveBeenCalled();
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
  expect(screen.getByRole('button', { name: 'Request deletion' })).toBeDisabled();
  fireEvent.click(screen.getByRole('checkbox'));
  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: 'Request deletion' }));
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

it.each([401, 403, 404])('does not declare success on a %s read failure', async (status) => {
  const client = agentClient({
    readSandbox: async () => {
      throw new ApiError(status, 'Unavailable', 'unavailable');
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

it('viewing another completed cleanup does not erase the retained request for a different resource', async () => {
  localStorage.setItem(deletionStorageKey('owner'), JSON.stringify(initial));
  render(
    <DeletionDialog
      viewerId="owner"
      client={agentClient({
        readSandbox: async () => ({ ...sandboxFixture('retired'), id: 'sbx_other' }),
      })}
      initial={{
        action: { kind: 'delete_sandbox', sandboxId: 'sbx_other' },
        key: 'view-other-cleanup',
        accepted: true,
      }}
      restored
      onChanged={vi.fn()}
      onClose={vi.fn()}
    />,
  );
  await screen.findByRole('dialog', { name: 'Sandbox deleted' });
  fireEvent.click(screen.getByRole('button', { name: 'Done' }));
  expect(restoreDeletion('owner')).toEqual(initial);
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

it('allows Workspace deletion with live Agents and shows the durable cascade steps', async () => {
  const observation = await observeDeletion(
    agentClient(),
    { kind: 'remove_workspace', workspaceId: 'wsp_1' },
    new AbortController().signal,
    false,
  );
  expect(observation.blocked).toBeUndefined();
  expect(observation.steps).toEqual([
    { label: 'Stop Agents', status: 'Waiting' },
    { label: 'Remove Agents and Worktrees', status: 'Waiting' },
    { label: 'Remove Workspace files', status: 'Waiting' },
    { label: 'Deletion confirmed', status: 'Waiting' },
  ]);
  localStorage.setItem(deletionStorageKey('owner'), '{broken');
  expect(restoreDeletion('owner')).toBeUndefined();
  expect(restoreDeletion('another-owner')).toBeUndefined();
});

it('reports child cleanup stages from refreshed Workspace records', async () => {
  for (const [agentStatus, workspaceStatus, statuses] of [
    ['ready', 'removing', ['In progress', 'Waiting', 'Waiting', 'Waiting']],
    ['stopped', 'removing', ['Done', 'In progress', 'Waiting', 'Waiting']],
    ['deleted', 'removing', ['Done', 'Done', 'In progress', 'Waiting']],
    ['deleted', 'removed', ['Done', 'Done', 'Done', 'Done']],
  ] as const) {
    const fixture = workspaceFixture(agentStatus);
    const client = agentClient({
      readWorkspace: async () => ({ ...fixture, status: workspaceStatus }),
    });
    const observation = await observeDeletion(
      client,
      { kind: 'remove_workspace', workspaceId: 'wsp_1' },
      new AbortController().signal,
      true,
    );
    expect(observation.steps.map((item) => item.status)).toEqual(statuses);
    expect(observation.complete).toBe(workspaceStatus === 'removed');
  }
});

it.each(['authorized', 'checkpoint', 'no-retire'] as const)(
  'guards revive deletion confirmation using a fresh %s observation',
  async (condition) => {
    const actor = userEvent.setup();
    const sandbox = sandboxFixture('reviving');
    const deleteSandbox = vi.fn(async () => undefined);
    render(
      <DeletionDialog
        viewerId="owner"
        client={agentClient({
          readSandbox: async () => ({
            ...sandbox,
            lifecycle: {
              ...sandbox.lifecycle!,
              operation: {
                purpose: 'revive',
                phase: 'conversation_reopen',
                startedAt: '2026-11-16T00:00:00Z',
              },
              result: {
                state: 'in_progress',
                operationId: 'revive-original',
                startedAt: '2026-11-16T00:00:00Z',
                resultDueAt: '2026-11-16T00:01:30Z',
                continuing: true,
                actions: condition === 'no-retire' ? ['observe'] : ['observe', 'retire'],
              },
              checkpoint:
                condition === 'checkpoint'
                  ? { phase: 'capture', startedAt: '2026-11-16T00:00:00Z', epoch: 1 }
                  : undefined,
            },
          }),
          deleteSandbox,
        })}
        initial={initial}
        restored={false}
        onChanged={vi.fn()}
        onClose={vi.fn()}
      />,
    );
    await actor.click(await screen.findByRole('checkbox'));
    const confirm = screen.getByRole('button', { name: 'Request deletion' });
    if (condition === 'authorized') {
      expect(confirm).toBeEnabled();
      await actor.click(confirm);
      expect(deleteSandbox).toHaveBeenCalledExactlyOnceWith('sbx_1', {
        idempotencyKey: initial.key,
      });
    } else {
      expect(confirm).toBeDisabled();
      expect(deleteSandbox).not.toHaveBeenCalled();
    }
  },
);
