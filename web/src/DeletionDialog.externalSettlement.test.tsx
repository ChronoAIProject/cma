import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { DeletionDialog } from './DeletionDialog';
import { useResourceActions } from './ResourceActions';
import { useDeletionNoticeReconciliation } from './useDeletionNoticeReconciliation';
import { persistDeletion, restoreDeletions } from './deletionRecovery';
import type { DeletionRequest } from './deletion';
import type { CmaWebClient } from './api/client';
import { AGENT_DETAIL, agentClient } from './test/agent';
import { sandboxFixture } from './test/hierarchy';

const request: DeletionRequest = {
  action: { kind: 'delete_agent', agentId: AGENT_DETAIL.id },
  key: 'review-delete',
  accepted: true,
  deleteSent: true,
  stop: {
    key: 'review-stop',
    phase: 'accepted',
    sandboxId: AGENT_DETAIL.sandbox.id,
    workspaceId: AGENT_DETAIL.workspace.id,
    worktreeId: AGENT_DETAIL.worktree.id,
  },
};
const terminal = {
  ...AGENT_DETAIL,
  status: 'deleted' as const,
  worktree: { ...AGENT_DETAIL.worktree, status: 'removed' as const },
};
beforeEach(() => {
  vi.useFakeTimers();
  localStorage.clear();
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.useRealTimers();
  localStorage.clear();
});

function fixture(target: 'Agent' | 'Sandbox' = 'Agent') {
  const readAgent = vi.fn().mockResolvedValue({ ...AGENT_DETAIL, status: 'stopped' });
  const readSandbox = vi.fn().mockResolvedValue({ ...sandboxFixture(), owner: 'owner' });
  const readAgentEvents = vi.fn().mockResolvedValue({ items: [] });
  const deleteAgent = vi.fn();
  const deleteSandbox = vi.fn();
  const stopAgent = vi.fn();
  const source = agentClient({
    readAgent,
    readSandbox,
    readAgentEvents,
    deleteAgent,
    deleteSandbox,
    stopAgent,
  });
  const currentRequest: DeletionRequest =
    target === 'Agent'
      ? request
      : {
          action: { kind: 'delete_sandbox', sandboxId: sandboxFixture().id },
          key: 'sandbox-delete',
          accepted: true,
          deleteSent: true,
        };
  let observedClient: CmaWebClient;
  function Actions() {
    const observed = useDeletionNoticeReconciliation(source, 'owner', 0, true);
    observedClient = observed.client;
    const actions = useResourceActions(
      observed.client,
      () => {},
      undefined,
      'owner',
      undefined,
      true,
      { stopAgent: true, deleteAgent: true },
      observed.settlements,
    );
    return (
      <>
        {actions.overlay}
        {actions.notice}
        <button disabled={!actions.viewDeletionStatus} onClick={actions.viewDeletionStatus}>
          Reopen request
        </button>
      </>
    );
  }
  return {
    target,
    request: currentRequest,
    readAgent,
    readSandbox,
    readAgentEvents,
    deleteAgent,
    deleteSandbox,
    stopAgent,
    Actions,
    externalRead: () =>
      target === 'Agent'
        ? observedClient.readAgent(AGENT_DETAIL.id)
        : observedClient.readSandbox(sandboxFixture().id),
  };
}

async function open(f: ReturnType<typeof fixture>, initial = f.request) {
  persistDeletion('owner', initial);
  render(<f.Actions />);
  await act(() => vi.advanceTimersByTimeAsync(0));
  if (initial.accepted) {
    fireEvent.click(screen.getByRole('button', { name: 'Reopen request' }));
    const progress = screen.getByRole('dialog', { name: 'Deletion progress' });
    fireEvent.click(
      within(progress).getByRole('button', {
        name: `View ${f.target === 'Sandbox' ? 'Sandbox' : 'Agent'} deletion status (1)`,
      }),
    );
    await act(() => vi.advanceTimersByTimeAsync(0));
  }
  expect(screen.getByRole('dialog')).toHaveAccessibleName(`Deleting ${f.target}`);
}

it('control: dialog-owned terminal read displays Done with no Stop/Delete writes', async () => {
  const f = fixture();
  await open(f);
  f.readAgent.mockResolvedValue(terminal);
  await act(() => vi.advanceTimersByTimeAsync(1500));
  expect(screen.getByRole('dialog')).toHaveAccessibleName('Agent deleted');
  expect(screen.getByRole('button', { name: 'Done' })).toBeEnabled();
  expect(restoreDeletions('owner')).toEqual([]);
  expect(f.deleteAgent).not.toHaveBeenCalled();
  expect(f.stopAgent).not.toHaveBeenCalled();
});

it.each([
  ['Agent', true],
  ['Agent', false],
  ['Sandbox', true],
  ['Sandbox', false],
] as const)(
  'external exact terminal read completes mounted %s dialog (initial accepted=%s)',
  async (target, accepted) => {
    const f = fixture(target);
    await open(f, { ...f.request, accepted });
    if (!accepted) persistDeletion('owner', f.request);
    f.readAgent.mockResolvedValue(terminal);
    f.readSandbox.mockResolvedValue({ ...sandboxFixture(), owner: 'owner', status: 'retired' });
    const writes = vi.spyOn(localStorage, 'setItem');
    await act(async () => {
      await f.externalRead();
    });
    expect(restoreDeletions('owner')).toEqual([]);
    expect(screen.getByRole('button', { name: 'Reopen request' })).toBeDisabled();
    const reads = [
      f.readAgent.mock.calls.length,
      f.readSandbox.mock.calls.length,
      f.readAgentEvents.mock.calls.length,
    ];
    await act(() => vi.advanceTimersByTimeAsync(15000));
    act(() => document.dispatchEvent(new Event('visibilitychange')));
    await act(() => vi.advanceTimersByTimeAsync(0));
    expect([
      f.readAgent.mock.calls.length,
      f.readSandbox.mock.calls.length,
      f.readAgentEvents.mock.calls.length,
    ]).toEqual(reads);
    expect(writes).not.toHaveBeenCalled();
    expect(f.deleteAgent).not.toHaveBeenCalled();
    expect(f.stopAgent).not.toHaveBeenCalled();
    expect(f.deleteSandbox).not.toHaveBeenCalled();
    expect(screen.getByRole('dialog')).toHaveAccessibleName(`${target} deleted`);
    expect(screen.getByRole('button', { name: 'Done' })).toBeEnabled();
    expect(screen.getByText(`${target} deletion is confirmed.`)).toBeVisible();
    expect(screen.queryByRole('button', { name: 'Check request' })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Done' }));
    await act(() => vi.advanceTimersByTimeAsync(1000));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(writes).not.toHaveBeenCalled();
    expect(restoreDeletions('owner')).toEqual([]);
  },
);

it('control: unacknowledged responsibility is not settled by external read', async () => {
  const f = fixture();
  const unknown = { ...request, accepted: false };
  await open(f, unknown);
  f.readAgent.mockResolvedValue(terminal);
  await act(async () => {
    await f.externalRead();
  });
  expect(restoreDeletions('owner')).toEqual([unknown]);
  expect(screen.getByRole('dialog')).toHaveAccessibleName('Deleting Agent');
  expect(f.deleteAgent).not.toHaveBeenCalled();
  expect(f.stopAgent).not.toHaveBeenCalled();
});

it('control: terminal detail with wrong Stop lineage cannot settle current responsibility', async () => {
  const f = fixture();
  await open(f);
  f.readAgent.mockResolvedValue({
    ...terminal,
    worktree: { ...terminal.worktree, id: 'unrelated-worktree' },
  });
  await act(async () => {
    await f.externalRead();
  });
  expect(restoreDeletions('owner')).toEqual([request]);
  expect(screen.getByRole('dialog')).toHaveAccessibleName('Deleting Agent');
  expect(f.deleteAgent).not.toHaveBeenCalled();
  expect(f.stopAgent).not.toHaveBeenCalled();
});

it.each(['delete key', 'Stop key', 'Stop lineage'] as const)(
  'does not complete the open request when external proof settles a replacement %s',
  async (field) => {
    const f = fixture();
    await open(f);
    localStorage.clear();
    const replacement: DeletionRequest = {
      ...request,
      key: field === 'delete key' ? 'replacement-delete' : request.key,
      stop: {
        ...request.stop!,
        key: field === 'Stop key' ? 'replacement-stop' : request.stop!.key,
        worktreeId: field === 'Stop lineage' ? 'replacement-worktree' : request.stop!.worktreeId,
      },
    };
    persistDeletion('owner', replacement);
    f.readAgent.mockResolvedValue({
      ...terminal,
      worktree: { ...terminal.worktree, id: replacement.stop!.worktreeId },
    });
    await act(async () => {
      await f.externalRead();
    });
    expect(restoreDeletions('owner')).toEqual([]);
    expect(screen.getByRole('dialog')).toHaveAccessibleName('Deleting Agent');
    expect(screen.queryByRole('button', { name: 'Done' })).not.toBeInTheDocument();
    expect(f.deleteAgent).not.toHaveBeenCalled();
    expect(f.stopAgent).not.toHaveBeenCalled();
    expect(f.deleteSandbox).not.toHaveBeenCalled();
  },
);

it('does not infer completion from a missing receipt', async () => {
  const f = fixture();
  await open(f);
  localStorage.clear();
  f.readAgent.mockResolvedValue(terminal);
  await act(async () => {
    await f.externalRead();
  });
  expect(screen.getByRole('dialog')).toHaveAccessibleName('Deleting Agent');
  expect(f.deleteAgent).not.toHaveBeenCalled();
  expect(f.stopAgent).not.toHaveBeenCalled();
});

it.each(['disabled', 'different viewer', 'different client'] as const)(
  'does not consume a terminal settlement with %s authority',
  async (scope) => {
    const client = agentClient();
    const otherClient = agentClient();
    const props = {
      client,
      viewerId: 'owner',
      initial: request,
      restored: true,
      onClose: vi.fn(),
      onChanged: vi.fn(),
      enabled: false,
    };
    const view = render(<DeletionDialog {...props} />);
    expect(screen.getByRole('dialog')).toHaveAccessibleName('Deleting Agent');
    view.rerender(
      <DeletionDialog
        {...props}
        enabled={scope !== 'disabled'}
        viewerId={scope === 'different viewer' ? 'other-owner' : 'owner'}
        client={scope === 'different client' ? otherClient : client}
        settlements={{ client, viewerId: 'owner', requests: [request] }}
      />,
    );
    await act(() => vi.advanceTimersByTimeAsync(0));
    expect(screen.getByRole('dialog')).toHaveAccessibleName('Deleting Agent');
    expect(screen.queryByRole('button', { name: 'Done' })).not.toBeInTheDocument();
  },
);

it.each(['Agent', 'Sandbox'] as const)(
  'verified %s settlement enables Done while explicit retry transport remains pending',
  async (target) => {
    const f = fixture(target);
    let resolveCommand!: () => void;
    const command = target === 'Agent' ? f.deleteAgent : f.deleteSandbox;
    command.mockImplementation(
      () =>
        new Promise<void>((resolve) => {
          resolveCommand = resolve;
        }),
    );
    await open(f, { ...f.request, accepted: false });
    fireEvent.click(screen.getByRole('button', { name: 'Check request' }));
    await act(() => vi.advanceTimersByTimeAsync(0));
    expect(command).toHaveBeenCalledTimes(1);
    expect(command.mock.calls[0]?.[1]).toMatchObject({ idempotencyKey: f.request.key });
    persistDeletion('owner', f.request);
    f.readAgent.mockResolvedValue(terminal);
    f.readSandbox.mockResolvedValue({ ...sandboxFixture(), owner: 'owner', status: 'retired' });
    const writes = vi.spyOn(localStorage, 'setItem');
    await act(async () => {
      await f.externalRead();
    });
    expect(restoreDeletions('owner')).toEqual([]);
    expect(screen.getByRole('dialog')).toHaveAccessibleName(`${target} deleted`);
    expect(screen.getByRole('button', { name: 'Done' })).toBeEnabled();
    expect(screen.queryByRole('button', { name: 'Check request' })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Done' }));
    await act(() => vi.advanceTimersByTimeAsync(1000));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    await act(async () => {
      resolveCommand();
    });
    await act(() => vi.advanceTimersByTimeAsync(15000));
    expect(command).toHaveBeenCalledTimes(1);
    expect(f.stopAgent).not.toHaveBeenCalled();
    expect(writes).not.toHaveBeenCalled();
    expect(restoreDeletions('owner')).toEqual([]);
  },
);
