import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ApiError } from '@cma/client';
import { useResourceActions } from './ResourceActions';
import { useDeletionNoticeReconciliation } from './useDeletionNoticeReconciliation';
import { persistDeletion, restoreDeletions } from './deletionRecovery';
import type { DeletionRequest } from './deletion';
import { AGENT_DETAIL, agentClient } from './test/agent';
import { sandboxFixture } from './test/hierarchy';

const request: DeletionRequest = {
  action: { kind: 'delete_agent', agentId: AGENT_DETAIL.id },
  key: 'completed-delete',
  accepted: false,
  deleteSent: true,
  stop: {
    key: 'completed-stop',
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
  vi.restoreAllMocks();
  vi.useRealTimers();
  localStorage.clear();
});

function fixture() {
  const readAgent = vi.fn().mockResolvedValue({ ...AGENT_DETAIL, status: 'stopped' });
  const readSandbox = vi.fn().mockResolvedValue({ ...sandboxFixture(), owner: 'owner' });
  const readAgentEvents = vi.fn().mockResolvedValue({ items: [] });
  const deleteAgent = vi.fn();
  const stopAgent = vi.fn();
  const source = agentClient({ readAgent, readSandbox, readAgentEvents, deleteAgent, stopAgent });
  let currentClient: typeof source;
  function Actions() {
    const observed = useDeletionNoticeReconciliation(source, 'owner', 0, true);
    currentClient = observed.client;
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
    readAgent,
    readSandbox,
    readAgentEvents,
    deleteAgent,
    stopAgent,
    Actions,
    readOutsideDialog: () => currentClient.readAgent(AGENT_DETAIL.id),
  };
}

it('shows confirmed deletion when another console read settles the open dialog', async () => {
  persistDeletion('owner', {
    action: { kind: 'delete_agent', agentId: AGENT_DETAIL.id },
    key: 'accepted-delete',
    accepted: true,
  });
  const { readAgent, readOutsideDialog, deleteAgent, stopAgent, Actions } = fixture();
  render(<Actions />);
  fireEvent.click(screen.getByRole('button', { name: 'Reopen request' }));
  await act(() => vi.advanceTimersByTimeAsync(0));
  expect(screen.getByRole('dialog')).toHaveAccessibleName('Deleting Agent');

  readAgent.mockResolvedValue(terminal);
  await act(async () => {
    await readOutsideDialog();
  });
  await act(() => vi.advanceTimersByTimeAsync(1500));

  expect(restoreDeletions('owner')).toEqual([]);
  expect(deleteAgent).not.toHaveBeenCalled();
  expect(stopAgent).not.toHaveBeenCalled();
  expect(screen.getByRole('dialog')).toHaveAccessibleName('Agent deleted');
});

it.each(['successful detail', 'parent refusal', 'history refusal'] as const)(
  'does not recreate a settled unknown receipt after polling returns %s',
  async (outcome) => {
    persistDeletion('owner', request);
    const { readAgent, readSandbox, readAgentEvents, deleteAgent, stopAgent, Actions } = fixture();
    const view = render(<Actions />);
    await act(() => vi.advanceTimersByTimeAsync(0));
    expect(screen.getByRole('dialog')).toHaveAccessibleName('Deleting Agent');
    expect(restoreDeletions('owner')).toEqual([request]);

    // A late acknowledgement changes durable responsibility while the already
    // mounted dialog still holds the original uncertain request.
    persistDeletion('owner', { ...request, accepted: true });
    readAgent.mockResolvedValue(terminal);
    const writes = vi.spyOn(localStorage, 'setItem');
    const removals = vi.spyOn(localStorage, 'removeItem');
    if (outcome === 'parent refusal')
      readSandbox.mockRejectedValue(new ApiError(403, 'Parent no longer accessible'));
    if (outcome === 'history refusal')
      readAgentEvents.mockRejectedValue(new ApiError(403, 'Events no longer accessible'));
    await act(() => vi.advanceTimersByTimeAsync(1500));

    // The production observer has accepted terminal Agent/Worktree evidence.
    expect(removals).toHaveBeenCalledWith('cma:stop-delete:v1:owner:completed-delete');
    expect(deleteAgent).not.toHaveBeenCalled();
    expect(stopAgent).not.toHaveBeenCalled();
    expect.soft(writes).not.toHaveBeenCalled();
    expect(restoreDeletions('owner')).toEqual([]);
    expect(screen.queryByRole('region', { name: 'Deletion request' })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Reopen request' })).toBeDisabled();
    const readCounts = [
      readAgent.mock.calls.length,
      readSandbox.mock.calls.length,
      readAgentEvents.mock.calls.length,
    ];
    await act(() => vi.advanceTimersByTimeAsync(15000));
    act(() => document.dispatchEvent(new Event('visibilitychange')));
    await act(() => vi.advanceTimersByTimeAsync(0));
    expect([
      readAgent.mock.calls.length,
      readSandbox.mock.calls.length,
      readAgentEvents.mock.calls.length,
    ]).toEqual(readCounts);
    expect(writes).not.toHaveBeenCalled();
    fireEvent.click(
      screen.getByText(outcome === 'successful detail' ? 'Done' : 'Close', { selector: 'button' }),
    );
    await act(() => vi.advanceTimersByTimeAsync(1000));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Reopen request' })).toBeDisabled();
    view.unmount();
    render(<Actions />);
    await act(() => vi.advanceTimersByTimeAsync(15000));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Reopen request' })).toBeDisabled();
    expect(restoreDeletions('owner')).toEqual([]);
    expect(writes).not.toHaveBeenCalled();
    expect(deleteAgent).not.toHaveBeenCalled();
    expect(stopAgent).not.toHaveBeenCalled();
  },
);

it.each([403, 404])(
  'retains an unknown request when parent read fails with %s without terminal proof',
  async (status) => {
    persistDeletion('owner', request);
    const f = fixture();
    f.readSandbox.mockRejectedValue(new ApiError(status, 'Parent not accessible'));
    const view = render(<f.Actions />);
    await act(() => vi.advanceTimersByTimeAsync(0));
    expect(restoreDeletions('owner')).toEqual([
      { ...request, stop: { ...request.stop!, phase: 'blocked' } },
    ]);
    fireEvent.click(screen.getByText('Close', { selector: 'button' }));
    await act(() => vi.advanceTimersByTimeAsync(1000));
    expect(screen.getByRole('button', { name: 'Reopen request' })).toBeEnabled();
    view.unmount();
    render(<f.Actions />);
    await act(() => vi.advanceTimersByTimeAsync(15000));
    expect(screen.getByRole('dialog')).toHaveAccessibleName('Deleting Agent');
    expect(restoreDeletions('owner')).toEqual([
      { ...request, stop: { ...request.stop!, phase: 'blocked' } },
    ]);
    expect(f.deleteAgent).not.toHaveBeenCalled();
    expect(f.stopAgent).not.toHaveBeenCalled();
  },
);
