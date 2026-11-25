import { StrictMode } from 'react';
import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ApiError } from '@cma/client';
import { DeletionDialog } from './DeletionDialog';
import type { DeletionRequest } from './deletion';
import { restoreDeletion } from './deletionRecovery';
import { coalesceDetailReads } from './api/detailReads';
import type { AgentDetail, AgentStatus, SandboxDetail } from './api/types';
import { AGENT_DETAIL, agentClient } from './test/agent';
import { sandboxFixture } from './test/hierarchy';

const initial: DeletionRequest = {
  action: { kind: 'delete_agent', agentId: AGENT_DETAIL.id },
  key: 'original-delete',
  accepted: false,
};
const grants = { stopAgent: true, deleteAgent: true };
beforeEach(() => vi.useFakeTimers());
afterEach(() => {
  vi.useRealTimers();
  localStorage.clear();
});
const tick = (ms = 0) => act(() => vi.advanceTimersByTimeAsync(ms));

function fixture(status: AgentStatus = 'working') {
  const state = { status, sandbox: { ...sandboxFixture(), owner: 'owner' } as SandboxDetail };
  const stop = vi.fn().mockResolvedValue(undefined);
  const remove = vi.fn().mockResolvedValue(undefined);
  const read = vi.fn(async () => ({ ...AGENT_DETAIL, status: state.status }));
  const client = agentClient({
    readAgent: read,
    readSandbox: async () => state.sandbox,
    stopAgent: stop,
    deleteAgent: remove,
  });
  const props = {
    client,
    viewerId: 'owner',
    initial,
    restored: false,
    agentPermissions: grants,
    onChanged: vi.fn(),
    onClose: vi.fn(),
  };
  return { state, stop, remove, read, props };
}

async function confirm() {
  await tick();
  fireEvent.click(screen.getByRole('checkbox'));
  fireEvent.click(screen.getByRole('button', { name: 'Stop and delete' }));
  await tick();
}

it('uses one confirmation and waits for confirmed stopped before deleting, including StrictMode', async () => {
  const f = fixture();
  let acceptStop!: () => void;
  f.stop.mockImplementation(
    () =>
      new Promise<void>((resolve) => {
        acceptStop = resolve;
      }),
  );
  render(
    <StrictMode>
      <DeletionDialog {...f.props} />
    </StrictMode>,
  );
  await tick();
  expect(f.stop).not.toHaveBeenCalled();
  expect(f.remove).not.toHaveBeenCalled();
  expect(screen.getByRole('button', { name: 'Stop and delete' })).toBeDisabled();
  await confirm();
  expect(f.stop).toHaveBeenCalledOnce();
  expect(f.remove).not.toHaveBeenCalled();
  await act(async () => acceptStop());
  await tick(3000);
  expect(f.remove).not.toHaveBeenCalled();
  expect(screen.getByText(/Waiting for the Agent to stop/)).toBeInTheDocument();
  const retained = restoreDeletion('owner')!;
  expect(retained.accepted).toBe(false);
  expect(retained.stop?.key).toBe(f.stop.mock.calls[0]![1].idempotencyKey);
  expect(retained.stop?.key).not.toBe(initial.key);
  f.state.status = 'stopped';
  await tick(1500);
  expect(f.remove).toHaveBeenCalledExactlyOnceWith(AGENT_DETAIL.id, {
    idempotencyKey: initial.key,
  });
  expect(restoreDeletion('owner')?.accepted).toBe(true);
  expect(screen.queryByText('Agent deletion is confirmed.')).not.toBeInTheDocument();
});

it('cancels combined confirmation without sending either command', async () => {
  const f = fixture();
  render(<DeletionDialog {...f.props} />);
  await tick();
  fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
  expect(f.props.onClose).toHaveBeenCalledOnce();
  expect(f.stop).not.toHaveBeenCalled();
  expect(f.remove).not.toHaveBeenCalled();
});

it('retains both original keys through uncertain Stop, reload and uncertain Delete retry', async () => {
  const f = fixture();
  f.stop.mockRejectedValueOnce(new ApiError(503, 'Stop response unavailable.'));
  f.remove.mockRejectedValueOnce(new ApiError(503, 'Delete response unavailable.'));
  const view = render(<DeletionDialog {...f.props} />);
  await confirm();
  const stopKey = f.stop.mock.calls[0]![1].idempotencyKey;
  const saved = restoreDeletion('owner')!;
  view.unmount();
  const resumed = render(<DeletionDialog {...f.props} initial={saved} restored />);
  await tick();
  expect(f.stop).toHaveBeenCalledOnce();
  expect(f.remove).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: 'Retry stop request' }));
  await tick();
  expect(f.stop).toHaveBeenLastCalledWith(AGENT_DETAIL.id, { idempotencyKey: stopKey });
  f.state.status = 'stopped';
  await tick(1500);
  expect(f.remove).toHaveBeenCalledOnce();
  await tick(6000);
  expect(f.remove).toHaveBeenCalledOnce();
  const uncertainDelete = restoreDeletion('owner')!;
  resumed.unmount();
  render(<DeletionDialog {...f.props} initial={uncertainDelete} restored />);
  await tick();
  expect(f.remove).toHaveBeenCalledOnce();
  fireEvent.click(screen.getByRole('button', { name: 'Check request' }));
  await tick();
  expect(f.remove).toHaveBeenCalledTimes(2);
  expect(f.remove.mock.calls[1]).toEqual(f.remove.mock.calls[0]);
  expect(f.stop).toHaveBeenCalledTimes(2);
  expect(restoreDeletion('owner')?.stop?.key).toBe(stopKey);
});

it.each(['failed', 'deleted'] as const)(
  'never treats %s after Stop as confirmed stopped, including reload',
  async (status) => {
    const f = fixture();
    const view = render(<DeletionDialog {...f.props} />);
    await confirm();
    f.state.status = status;
    await tick(3000);
    expect(f.remove).not.toHaveBeenCalled();
    const saved = restoreDeletion('owner')!;
    expect(saved.stop?.phase).toBe('blocked');
    view.unmount();
    f.state.status = 'stopped';
    render(<DeletionDialog {...f.props} initial={saved} restored />);
    await tick();
    expect(screen.getByRole('button', { name: 'Check request' })).toBeDisabled();
    fireEvent.click(screen.getByRole('button', { name: 'Check request' }));
    await tick();
    expect(f.remove).not.toHaveBeenCalled();
    expect(screen.getByText(/Stopping has not been confirmed/)).toBeInTheDocument();
  },
);

it.each(['owner', 'parent', 'permission', 'unmount'] as const)(
  'does not continue to Delete after %s changes while stopping',
  async (change) => {
    const f = fixture();
    const view = render(<DeletionDialog {...f.props} />);
    await confirm();
    if (change === 'owner') f.state.sandbox = { ...f.state.sandbox, owner: 'someone-else' };
    if (change === 'parent') f.state.sandbox = { ...f.state.sandbox, status: 'rotating' };
    if (change === 'permission')
      view.rerender(
        <DeletionDialog {...f.props} agentPermissions={{ ...grants, deleteAgent: false }} />,
      );
    if (change === 'unmount') view.unmount();
    f.state.status = 'stopped';
    await tick(3000);
    expect(f.remove).not.toHaveBeenCalled();
  },
);

it.each(['stopped', 'failed'] as const)(
  'keeps direct deletion for an initially %s Agent',
  async (status) => {
    const f = fixture(status);
    render(<DeletionDialog {...f.props} />);
    await tick();
    fireEvent.click(screen.getByRole('checkbox'));
    fireEvent.click(screen.getByRole('button', { name: 'Delete Agent' }));
    await tick();
    expect(f.stop).not.toHaveBeenCalled();
    expect(f.remove).toHaveBeenCalledExactlyOnceWith(AGENT_DETAIL.id, {
      idempotencyKey: initial.key,
    });
  },
);

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

it('does not authorize Delete from a coalesced read started before Stop settled', async () => {
  const f = fixture();
  const accepted = deferred<void>();
  f.stop.mockReturnValue(accepted.promise);
  const wrapped = coalesceDetailReads(f.props.client);
  render(<DeletionDialog {...f.props} client={wrapped.client} />);
  await confirm();
  expect(f.stop).toHaveBeenCalledOnce();
  const older = deferred<AgentDetail>();
  f.read.mockImplementationOnce(() => older.promise);
  const stale = wrapped.client.readAgent(AGENT_DETAIL.id);
  await tick();
  await act(async () => accepted.resolve());
  await tick();
  await act(async () => {
    older.resolve({ ...AGENT_DETAIL, status: 'stopped' });
    await stale;
  });
  await tick();
  expect(f.remove).not.toHaveBeenCalled();
  f.state.status = 'stopped';
  await tick(1500);
  expect(f.remove).toHaveBeenCalledOnce();
  wrapped.dispose();
});

it.each(['viewer', 'client', 'disabled', 'stop grant', 'unmount'] as const)(
  'ignores a late Stop response after %s changes and retains the original receipt',
  async (change) => {
    const f = fixture();
    const accepted = deferred<void>();
    f.stop.mockReturnValue(accepted.promise);
    const view = render(<DeletionDialog {...f.props} />);
    await confirm();
    const saved = restoreDeletion('owner')!;
    if (change === 'viewer') view.rerender(<DeletionDialog {...f.props} viewerId="other" />);
    if (change === 'client') view.rerender(<DeletionDialog {...f.props} client={agentClient()} />);
    if (change === 'disabled') view.rerender(<DeletionDialog {...f.props} enabled={false} />);
    if (change === 'stop grant')
      view.rerender(
        <DeletionDialog {...f.props} agentPermissions={{ ...grants, stopAgent: false }} />,
      );
    if (change === 'unmount') view.unmount();
    f.state.status = 'stopped';
    await act(async () => accepted.resolve());
    await tick(3000);
    expect(f.remove).not.toHaveBeenCalled();
    expect(restoreDeletion('owner')).toMatchObject({
      key: saved.key,
      stop: { key: saved.stop!.key },
    });
    expect(restoreDeletion('other')).toBeUndefined();
    if (change !== 'unmount') expect(restoreDeletion('owner')?.stop?.phase).toBe('blocked');
  },
);

it.each(['agent', 'sandbox', 'workspace', 'worktree', '404'] as const)(
  'blocks a %s mismatch after Stop, keeps keys, and cannot retry as a direct Delete',
  async (change) => {
    const f = fixture();
    const view = render(<DeletionDialog {...f.props} />);
    await confirm();
    const saved = restoreDeletion('owner')!;
    const stopped: AgentDetail = { ...AGENT_DETAIL, status: 'stopped' };
    if (change === 'agent') f.read.mockResolvedValue({ ...stopped, id: 'other' });
    if (change === 'sandbox')
      f.read.mockResolvedValue({ ...stopped, sandbox: { ...stopped.sandbox, id: 'other' } });
    if (change === 'workspace')
      f.read.mockResolvedValue({
        ...stopped,
        workspace: { ...stopped.workspace, id: 'other' },
        worktree: { ...stopped.worktree, workspaceId: 'other' },
      });
    if (change === 'worktree')
      f.read.mockResolvedValue({ ...stopped, worktree: { ...stopped.worktree, id: 'other' } });
    if (change === '404') f.read.mockRejectedValue(new ApiError(404, 'Not found'));
    await tick(3000);
    const blocked = restoreDeletion('owner')!;
    expect(blocked).toMatchObject({
      key: saved.key,
      stop: { key: saved.stop!.key, phase: 'blocked' },
    });
    expect(f.remove).not.toHaveBeenCalled();
    view.unmount();
    f.read.mockResolvedValue(stopped);
    render(<DeletionDialog {...f.props} initial={blocked} restored />);
    await tick();
    expect(screen.getByRole('button', { name: 'Check request' })).toBeDisabled();
    expect(f.remove).not.toHaveBeenCalled();
  },
);

it.each([new DOMException('signal is aborted without reason', 'AbortError'), new Error('timeout')])(
  'does not treat an uncertain Stop response as stopped: %s',
  async (error) => {
    const f = fixture();
    f.stop.mockRejectedValue(error);
    render(<DeletionDialog {...f.props} />);
    await confirm();
    await tick(3000);
    expect(f.remove).not.toHaveBeenCalled();
    expect(restoreDeletion('owner')?.stop?.phase).toBe('requested');
    expect(screen.queryByText(/signal is aborted/)).not.toBeInTheDocument();
  },
);

it.each([400, 401, 403, 404, 409, 422])(
  'retains a blocked combined receipt after Stop refusal %s',
  async (status) => {
    const f = fixture();
    f.stop.mockRejectedValue(new ApiError(status, 'Stop refused'));
    render(<DeletionDialog {...f.props} />);
    await confirm();
    expect(restoreDeletion('owner')?.stop?.phase).toBe('blocked');
    f.state.status = 'stopped';
    await tick(3000);
    expect(f.remove).not.toHaveBeenCalled();
  },
);

it('requires Stop access for a live Agent but preserves terminal direct Delete without it', async () => {
  const f = fixture();
  const props = { ...f.props, agentPermissions: { ...grants, stopAgent: false } };
  const view = render(<DeletionDialog {...props} />);
  await tick();
  expect(screen.getByRole('button', { name: 'Stop and delete' })).toBeDisabled();
  expect(screen.getByText(/Your account cannot stop/)).toBeInTheDocument();
  view.unmount();
  f.state.status = 'stopped';
  render(<DeletionDialog {...props} />);
  await tick();
  fireEvent.click(screen.getByRole('checkbox'));
  fireEvent.click(screen.getByRole('button', { name: 'Delete Agent' }));
  await tick();
  expect(f.stop).not.toHaveBeenCalled();
  expect(f.remove).toHaveBeenCalledOnce();
});

it('retains accepted deletion until both Agent deletion and Worktree removal are observed', async () => {
  const f = fixture();
  render(<DeletionDialog {...f.props} />);
  await confirm();
  f.state.status = 'stopped';
  await tick(1500);
  f.state.status = 'deleted';
  await tick(1500);
  expect(restoreDeletion('owner')?.accepted).toBe(true);
  f.read.mockResolvedValue({
    ...AGENT_DETAIL,
    status: 'deleted',
    worktree: { ...AGENT_DETAIL.worktree, status: 'removed' },
  });
  await tick(3000);
  expect(restoreDeletion('owner')).toBeUndefined();
  expect(screen.getByText('Agent deletion is confirmed.')).toBeInTheDocument();
});

it.each(['unmount', 'client', 'viewer', 'permission'] as const)(
  'does not Delete when %s changes during the final stopped read',
  async (change) => {
    const f = fixture();
    const view = render(<DeletionDialog {...f.props} />);
    await confirm();
    const finalRead = deferred<AgentDetail>();
    f.read.mockImplementationOnce(() => finalRead.promise);
    fireEvent.click(screen.getByRole('button', { name: 'Check request' }));
    await tick();
    if (change === 'unmount') view.unmount();
    if (change === 'client') view.rerender(<DeletionDialog {...f.props} client={agentClient()} />);
    if (change === 'viewer') view.rerender(<DeletionDialog {...f.props} viewerId="other" />);
    if (change === 'permission')
      view.rerender(
        <DeletionDialog {...f.props} agentPermissions={{ ...grants, deleteAgent: false }} />,
      );
    await act(async () => finalRead.resolve({ ...AGENT_DETAIL, status: 'stopped' }));
    await tick(3000);
    expect(f.remove).not.toHaveBeenCalled();
    expect(restoreDeletion('owner')?.key).toBe(initial.key);
  },
);

it('closing after Stop preserves consent and keys but requires an explicit check to continue on reopen', async () => {
  const f = fixture();
  const view = render(<DeletionDialog {...f.props} />);
  await confirm();
  const saved = restoreDeletion('owner')!;
  fireEvent.click(screen.getByText('Close', { selector: 'button' }));
  f.state.status = 'stopped';
  await tick(3000);
  expect(f.remove).not.toHaveBeenCalled();
  view.unmount();
  render(<DeletionDialog {...f.props} initial={saved} restored />);
  await tick(3000);
  expect(f.remove).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: 'Check request' }));
  await tick();
  expect(f.remove).toHaveBeenCalledExactlyOnceWith(AGENT_DETAIL.id, { idempotencyKey: saved.key });
  expect(f.stop).toHaveBeenCalledOnce();
});

it('does not upgrade a restored ordinary Delete request into Stop consent', async () => {
  const f = fixture();
  render(<DeletionDialog {...f.props} restored />);
  await tick();
  fireEvent.click(screen.getByRole('button', { name: 'Check request' }));
  await tick();
  expect(f.stop).not.toHaveBeenCalled();
  expect(f.remove).not.toHaveBeenCalled();
  expect(screen.getByText(/cannot continue until the Agent is stopped/)).toBeInTheDocument();
});

it('only explicitly retries uncertain Stop when a stopped poll is withdrawn by a fresh working read', async () => {
  const f = fixture();
  f.stop.mockRejectedValueOnce(new ApiError(503, 'Stop reply unknown'));
  render(<DeletionDialog {...f.props} />);
  await confirm();
  const original = restoreDeletion('owner')!;
  expect(f.stop).toHaveBeenCalledOnce();
  expect(original.stop?.phase).toBe('requested');
  f.read.mockResolvedValueOnce({ ...AGENT_DETAIL, status: 'stopped' });
  await tick(1500);
  expect(f.remove).not.toHaveBeenCalled();
  expect(f.stop).toHaveBeenCalledOnce();
  expect(screen.getByText('Stop reply unknown')).toBeInTheDocument();
  expect(restoreDeletion('owner')).toEqual(original);
  fireEvent.click(screen.getByRole('button', { name: 'Retry stop request' }));
  await tick();
  expect(f.stop).toHaveBeenCalledTimes(2);
  expect(f.stop.mock.calls[1]).toEqual(f.stop.mock.calls[0]);
  expect(f.remove).not.toHaveBeenCalled();
  await tick(4500);
  expect(f.stop).toHaveBeenCalledTimes(2);
  f.state.status = 'stopped';
  await tick(1500);
  expect(f.remove).toHaveBeenCalledExactlyOnceWith(AGENT_DETAIL.id, {
    idempotencyKey: original.key,
  });
});

it('requires an explicit check after an uncertain Stop even when polling later confirms stopped', async () => {
  const f = fixture();
  f.stop.mockRejectedValueOnce(new ApiError(503, 'Stop reply unknown'));
  render(<DeletionDialog {...f.props} />);
  await confirm();
  const original = restoreDeletion('owner')!;
  f.state.status = 'stopped';
  await tick(3000);
  expect(f.stop).toHaveBeenCalledOnce();
  expect(f.remove).not.toHaveBeenCalled();
  expect(restoreDeletion('owner')).toEqual(original);
  fireEvent.click(screen.getByRole('button', { name: 'Retry stop request' }));
  await tick();
  expect(f.stop).toHaveBeenCalledOnce();
  expect(f.remove).toHaveBeenCalledExactlyOnceWith(AGENT_DETAIL.id, {
    idempotencyKey: original.key,
  });
  expect(restoreDeletion('owner')?.stop?.key).toBe(original.stop?.key);
});
