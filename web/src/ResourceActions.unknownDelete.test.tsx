import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ApiError } from '@cma/client';
import { useResourceActions } from './ResourceActions';
import { useDeletionNoticeReconciliation } from './useDeletionNoticeReconciliation';
import { AGENT_DETAIL, agentClient } from './test/agent';
import { sandboxFixture, workspaceFixture } from './test/hierarchy';
import type { AgentStatus, SandboxDetail } from './api/types';
import { persistDeletion, restoreDeletions } from './deletionRecovery';

beforeEach(() => vi.useFakeTimers());
afterEach(() => {
  localStorage.clear();
  vi.useRealTimers();
});
const tick = (ms = 0) => act(() => vi.advanceTimersByTimeAsync(ms));
const click = async (name: string) => {
  fireEvent.click(screen.getByRole('button', { name }));
  await tick();
};

function fixture() {
  const state = {
    status: 'working' as AgentStatus,
    sandbox: { ...sandboxFixture(), owner: 'owner' } as SandboxDetail,
  };
  const stop = vi.fn().mockResolvedValue(undefined);
  const remove = vi.fn().mockResolvedValue(undefined);
  const removeParent = vi.fn().mockResolvedValue(undefined);
  const removeWorkspace = vi.fn().mockResolvedValue(undefined);
  const read = vi.fn(async (id: string) => ({
    ...AGENT_DETAIL,
    id,
    status: state.status,
    worktree: { ...AGENT_DETAIL.worktree, agentId: id },
  }));
  const readParent = vi.fn(async () => state.sandbox);
  const client = agentClient({
    readAgent: read,
    readSandbox: readParent,
    readWorkspace: async () => workspaceFixture('stopped'),
    stopAgent: stop,
    deleteAgent: remove,
    deleteSandbox: removeParent,
    removeWorkspace,
  });
  function Actions({ allowDelete = true }: { allowDelete?: boolean }) {
    const observed = useDeletionNoticeReconciliation(client, 'owner', 0, true);
    const actions = useResourceActions(
      observed.client,
      vi.fn(),
      undefined,
      'owner',
      undefined,
      true,
      { stopAgent: true, deleteAgent: allowDelete },
      observed.settlements,
    );
    return (
      <>
        <button
          disabled={actions.pending}
          onClick={() => actions.select({ kind: 'delete_agent', agentId: AGENT_DETAIL.id })}
        >
          Delete original Agent
        </button>
        <button
          disabled={actions.pending}
          onClick={() => actions.select({ kind: 'delete_agent', agentId: 'agt_other' })}
        >
          Delete other Agent
        </button>
        <button
          disabled={actions.pending}
          onClick={() => actions.select({ kind: 'delete_sandbox', sandboxId: state.sandbox.id })}
        >
          Delete parent environment
        </button>
        <button
          disabled={actions.pending}
          onClick={() =>
            actions.select({ kind: 'remove_workspace', workspaceId: AGENT_DETAIL.workspace.id })
          }
        >
          Delete Workspace
        </button>
        {actions.overlay}
        {actions.notice}
      </>
    );
  }
  return { state, stop, remove, read, readParent, removeParent, removeWorkspace, Actions };
}

async function close() {
  fireEvent.click(screen.getByText('Close', { selector: 'button' }));
  await tick(1000);
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
}

async function start() {
  await click('Delete original Agent');
  fireEvent.click(screen.getByRole('checkbox'));
  await click('Stop and delete');
}

it.each([
  ['parent', 'End this request'],
  ['parent', 'Close'],
  ['permission', 'End this request'],
  ['permission', 'Close'],
] as const)(
  'retains this unknown Delete across %s loss, %s, reload and explicit same-key retry',
  async (loss, exit) => {
    const f = fixture();
    f.remove.mockRejectedValueOnce(new ApiError(503, 'Delete reply unknown'));
    const view = render(<f.Actions />);
    await start();
    f.state.status = 'stopped';
    await tick(1500);
    expect(f.remove).toHaveBeenCalledOnce();
    const uncertain = restoreDeletions('owner')[0]!;
    if (loss === 'parent') f.state.sandbox = { ...f.state.sandbox, status: 'rotating' };
    else view.rerender(<f.Actions allowDelete={false} />);
    await tick(1500);
    expect(restoreDeletions('owner')[0]?.stop?.phase).toBe('blocked');
    if (exit === 'Close') await close();
    else {
      await click(exit);
      await tick(1000);
    }
    expect(restoreDeletions('owner')).toEqual([
      expect.objectContaining({
        key: uncertain.key,
        accepted: false,
        stop: expect.objectContaining({ key: uncertain.stop!.key }),
      }),
    ]);
    expect(f.remove).toHaveBeenCalledOnce();
    view.unmount();
    const reloaded = render(<f.Actions allowDelete={loss !== 'permission'} />);
    await tick();
    expect(screen.getByRole('dialog')).toHaveAccessibleName('Deleting Agent');
    await close();
    if (loss === 'permission') reloaded.rerender(<f.Actions />);
    await click('Delete parent environment');
    expect(screen.getByRole('dialog')).toHaveAccessibleName('Delete Sandbox');
    expect(screen.getByRole('checkbox')).not.toBeChecked();
    await click('Keep environment');
    await tick(1000);
    f.state.sandbox = { ...f.state.sandbox, status: 'active' };
    await click('Delete original Agent');
    expect(screen.queryByRole('checkbox')).not.toBeInTheDocument();
    expect(f.remove).toHaveBeenCalledOnce();
    await click('Check request');
    expect(f.remove).toHaveBeenCalledTimes(2);
    expect(f.remove.mock.calls[1]).toEqual(f.remove.mock.calls[0]);
    expect(f.stop).toHaveBeenCalledOnce();
    expect(restoreDeletions('owner')[0]).toMatchObject({ key: uncertain.key, accepted: true });
  },
);

it.each([
  ['accepted', 'parent'],
  ['unknown', 'parent'],
  ['accepted', 'permission'],
  ['unknown', 'permission'],
] as const)(
  'keeps the key when ending a blocked pending Delete whose late reply is %s after %s loss',
  async (reply, loss) => {
    const f = fixture();
    let accept!: () => void;
    let reject!: (error: Error) => void;
    f.remove.mockImplementationOnce(
      () =>
        new Promise<void>((resolve, fail) => {
          accept = resolve;
          reject = fail;
        }),
    );
    const view = render(<f.Actions />);
    await start();
    f.state.status = 'stopped';
    await tick(1500);
    expect(f.remove).toHaveBeenCalledOnce();
    const pending = restoreDeletions('owner')[0]!;
    if (loss === 'parent') f.state.sandbox = { ...f.state.sandbox, status: 'rotating' };
    else view.rerender(<f.Actions allowDelete={false} />);
    await tick(1500);
    await click('End this request');
    await tick(1000);
    expect(restoreDeletions('owner')[0]?.key).toBe(pending.key);
    await act(async () =>
      reply === 'accepted' ? accept() : reject(new ApiError(503, 'Delete reply unknown')),
    );
    await tick();
    expect(restoreDeletions('owner')[0]).toMatchObject({
      key: pending.key,
      accepted: reply === 'accepted',
    });
    expect(f.remove).toHaveBeenCalledOnce();
    if (reply === 'accepted') {
      await click('Delete original Agent');
      expect(screen.queryByRole('button', { name: 'End this request' })).not.toBeInTheDocument();
      expect(screen.getByText(/Delete request accepted/)).toBeInTheDocument();
      await close();
    }
    view.unmount();
    f.state.sandbox = { ...f.state.sandbox, status: 'active' };
    render(<f.Actions />);
    await tick();
    if (reply === 'accepted') await click('Delete original Agent');
    expect(screen.getByRole('dialog')).toHaveAccessibleName('Deleting Agent');
    expect(screen.queryByRole('checkbox')).not.toBeInTheDocument();
    expect(f.remove).toHaveBeenCalledOnce();
    if (reply === 'accepted')
      expect(screen.queryByRole('button', { name: 'End this request' })).not.toBeInTheDocument();
    else {
      await click('Check request');
      expect(f.remove.mock.calls[1]).toEqual(f.remove.mock.calls[0]);
      expect(f.stop).toHaveBeenCalledOnce();
    }
  },
);

it('does not resurrect a completed receipt when an old Delete reply arrives after End', async () => {
  const f = fixture();
  let accept!: () => void;
  f.remove.mockImplementationOnce(
    () =>
      new Promise<void>((resolve) => {
        accept = resolve;
      }),
  );
  const view = render(<f.Actions />);
  await start();
  f.state.status = 'stopped';
  await tick(1500);
  f.state.sandbox = { ...f.state.sandbox, status: 'rotating' };
  await tick(1500);
  await click('End this request');
  await tick(1000);
  expect(restoreDeletions('owner')).toHaveLength(1);
  view.unmount();
  f.state.sandbox = { ...f.state.sandbox, status: 'active' };
  f.read.mockResolvedValue({
    ...AGENT_DETAIL,
    status: 'deleted',
    worktree: { ...AGENT_DETAIL.worktree, agentId: AGENT_DETAIL.id, status: 'removed' },
  });
  render(<f.Actions />);
  await tick();
  expect(restoreDeletions('owner')).toEqual([]);
  await act(async () => accept());
  expect(restoreDeletions('owner')).toEqual([]);
  expect(f.remove).toHaveBeenCalledOnce();
});

it('retains older combined receipts whose Delete dispatch was not recorded', async () => {
  const f = fixture();
  const older = {
    action: { kind: 'delete_agent' as const, agentId: AGENT_DETAIL.id },
    key: 'old-delete-key',
    accepted: false,
    stop: {
      key: 'old-stop-key',
      phase: 'blocked' as const,
      sandboxId: AGENT_DETAIL.sandbox.id,
      workspaceId: AGENT_DETAIL.workspace.id,
      worktreeId: AGENT_DETAIL.worktree.id,
    },
  };
  persistDeletion('owner', older);
  render(<f.Actions />);
  await tick();
  await click('End this request');
  await tick(1000);
  expect(restoreDeletions('owner')).toEqual([older]);
  f.state.status = 'stopped';
  await click('Delete original Agent');
  expect(screen.queryByRole('checkbox')).not.toBeInTheDocument();
  await click('Check request');
  expect(f.remove).toHaveBeenCalledExactlyOnceWith(AGENT_DETAIL.id, { idempotencyKey: older.key });
  expect(f.stop).not.toHaveBeenCalled();
});

it('a late unknown reply cannot downgrade acceptance from an explicit retry of the original key', async () => {
  const f = fixture();
  let reject!: (error: Error) => void;
  f.remove.mockImplementationOnce(
    () =>
      new Promise<void>((_, fail) => {
        reject = fail;
      }),
  );
  render(<f.Actions />);
  await start();
  f.state.status = 'stopped';
  await tick(1500);
  const sent = restoreDeletions('owner')[0]!;
  f.state.sandbox = { ...f.state.sandbox, status: 'rotating' };
  await tick(1500);
  await click('End this request');
  await tick(1000);
  f.state.sandbox = { ...f.state.sandbox, status: 'active' };
  await click('Delete original Agent');
  await click('Check request');
  expect(restoreDeletions('owner')[0]).toMatchObject({ key: sent.key, accepted: true });
  expect(f.remove.mock.calls[1]).toEqual(f.remove.mock.calls[0]);
  await act(async () => reject(new ApiError(503, 'Old reply unknown')));
  expect(restoreDeletions('owner')[0]).toMatchObject({ key: sent.key, accepted: true });
});

it('does not retry Delete from a stale preflight after a newer parent refusal', async () => {
  const f = fixture();
  f.remove.mockRejectedValueOnce(new ApiError(503, 'Delete reply unknown'));
  render(<f.Actions />);
  await start();
  f.state.status = 'stopped';
  await tick(1500);
  expect(f.remove).toHaveBeenCalledOnce();
  const original = restoreDeletions('owner')[0]!;
  const oldParent = f.state.sandbox;
  let finish!: (parent: SandboxDetail) => void;
  f.readParent.mockImplementationOnce(
    () =>
      new Promise<SandboxDetail>((resolve) => {
        finish = resolve;
      }),
  );
  await click('Check request');
  f.state.sandbox = { ...f.state.sandbox, status: 'rotating' };
  await tick(1500);
  expect(restoreDeletions('owner')[0]?.stop?.phase).toBe('blocked');
  await act(async () => finish(oldParent));
  await tick();
  expect(f.remove).toHaveBeenCalledOnce();
  expect(restoreDeletions('owner')[0]?.key).toBe(original.key);
});

it.each(['before reopen', 'after reopen', 'during Agent read', 'during parent read'] as const)(
  'keeps late acceptance authoritative when it arrives %s',
  async (arrival) => {
    const f = fixture();
    let accept!: () => void;
    f.remove.mockImplementationOnce(
      () =>
        new Promise<void>((resolve) => {
          accept = resolve;
        }),
    );
    f.remove.mockRejectedValueOnce(new ApiError(503, 'Retry reply unknown'));
    const view = render(<f.Actions />);
    await start();
    f.state.status = 'stopped';
    await tick(1500);
    const sent = restoreDeletions('owner')[0]!;
    expect(f.remove).toHaveBeenCalledOnce();
    f.state.sandbox = { ...f.state.sandbox, status: 'rotating' };
    await tick(1500);
    await click('End this request');
    await tick(1000);
    f.state.sandbox = { ...f.state.sandbox, status: 'active' };
    if (arrival !== 'before reopen') await click('Delete original Agent');
    let finishPreflight: (() => void) | undefined;
    if (arrival.startsWith('during')) {
      const pendingRead = new Promise<void>((resolve) => {
        finishPreflight = resolve;
      });
      if (arrival === 'during Agent read')
        f.read.mockImplementationOnce(async (id) => {
          await pendingRead;
          return {
            ...AGENT_DETAIL,
            id,
            status: 'stopped',
            worktree: { ...AGENT_DETAIL.worktree, agentId: id },
          };
        });
      else
        f.readParent.mockImplementationOnce(async () => {
          await pendingRead;
          return f.state.sandbox;
        });
      await click('Check request');
    }
    await act(async () => accept());
    await tick();
    expect(restoreDeletions('owner')[0]).toMatchObject({ key: sent.key, accepted: true });
    if (finishPreflight) {
      await act(finishPreflight);
      await tick();
    } else {
      if (arrival === 'before reopen') await click('Delete original Agent');
      const check = screen.queryByRole('button', { name: 'Check request' });
      if (check) {
        fireEvent.click(check);
        await tick();
      }
    }
    expect(f.remove).toHaveBeenCalledOnce();
    expect(f.stop).toHaveBeenCalledOnce();
    expect(restoreDeletions('owner')[0]).toMatchObject({ key: sent.key, accepted: true });
    expect(screen.queryByRole('button', { name: 'Check request' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'End this request' })).not.toBeInTheDocument();
    expect(screen.getByText(/Delete request accepted/)).toBeInTheDocument();
    await close();
    view.unmount();
    render(<f.Actions />);
    await click('Delete original Agent');
    expect(screen.getByText(/Delete request accepted/)).toBeInTheDocument();
    expect(f.remove).toHaveBeenCalledOnce();
  },
);

it('does not recreate a completed receipt when retry preflight finishes after cleanup', async () => {
  const f = fixture();
  let accept!: () => void;
  f.remove.mockImplementationOnce(
    () =>
      new Promise<void>((resolve) => {
        accept = resolve;
      }),
  );
  render(<f.Actions />);
  await start();
  f.state.status = 'stopped';
  await tick(1500);
  f.state.sandbox = { ...f.state.sandbox, status: 'rotating' };
  await tick(1500);
  await click('End this request');
  await tick(1000);
  f.state.sandbox = { ...f.state.sandbox, status: 'active' };
  await click('Delete original Agent');
  let finish!: () => void;
  const pendingRead = new Promise<void>((resolve) => {
    finish = resolve;
  });
  f.read.mockImplementationOnce(async (id) => {
    await pendingRead;
    return {
      ...AGENT_DETAIL,
      id,
      status: 'stopped',
      worktree: { ...AGENT_DETAIL.worktree, agentId: id },
    };
  });
  await click('Check request');
  f.read.mockResolvedValue({
    ...AGENT_DETAIL,
    status: 'deleted',
    worktree: { ...AGENT_DETAIL.worktree, agentId: AGENT_DETAIL.id, status: 'removed' },
  });
  await tick(1500);
  expect(restoreDeletions('owner')).toEqual([]);
  expect(screen.getByRole('dialog')).toHaveAccessibleName('Agent deleted');
  await act(async () => accept());
  await act(async () => finish());
  await tick();
  expect(restoreDeletions('owner')).toEqual([]);
  expect(f.remove).toHaveBeenCalledOnce();
  expect(f.stop).toHaveBeenCalledOnce();
  await click('Done');
  await tick(1000);
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
});
