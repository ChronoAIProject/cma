import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ApiError } from '@cma/client';
import { useResourceActions } from './ResourceActions';
import { AGENT_DETAIL, agentClient } from './test/agent';
import { sandboxFixture, workspaceFixture } from './test/hierarchy';
import type { AgentStatus, SandboxDetail } from './api/types';
import type { DeletionRequest } from './deletion';
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
const blocked: DeletionRequest = {
  action: { kind: 'delete_agent', agentId: AGENT_DETAIL.id },
  key: 'delete-original',
  accepted: false,
  deleteSent: false,
  stop: {
    key: 'stop-original',
    phase: 'blocked',
    sandboxId: AGENT_DETAIL.sandbox.id,
    workspaceId: AGENT_DETAIL.workspace.id,
    worktreeId: AGENT_DETAIL.worktree.id,
  },
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
  const client = agentClient({
    readAgent: async (id) => ({
      ...AGENT_DETAIL,
      id,
      status: state.status,
      worktree: { ...AGENT_DETAIL.worktree, agentId: id },
    }),
    readSandbox: async () => state.sandbox,
    readWorkspace: async () => workspaceFixture('stopped'),
    stopAgent: stop,
    deleteAgent: remove,
    deleteSandbox: removeParent,
    removeWorkspace,
  });
  function Actions() {
    const actions = useResourceActions(client, vi.fn(), undefined, 'owner', undefined, true, {
      stopAgent: true,
      deleteAgent: true,
    });
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
  return { state, stop, remove, removeParent, removeWorkspace, Actions };
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
  ['Delete parent environment', 'Delete Sandbox'],
  ['Delete other Agent', 'Delete Agent'],
  ['Delete Workspace', 'Delete Workspace'],
])('a retained blocked Stop request does not intercept %s', async (action, title) => {
  const f = fixture();
  persistDeletion('owner', blocked);
  const view = render(<f.Actions />);
  await tick();
  expect(screen.getByRole('button', { name: 'Check request' })).toBeDisabled();
  await close();
  view.unmount();
  render(<f.Actions />);
  await tick();
  expect(screen.getByRole('dialog')).toHaveAccessibleName('Deleting Agent');
  await close();
  await click(action);
  expect(screen.getByRole('dialog')).toHaveAccessibleName(title);
  expect(screen.getByRole('checkbox')).not.toBeChecked();
  expect(restoreDeletions('owner')).toEqual([blocked]);
  expect(f.stop).not.toHaveBeenCalled();
  expect(f.remove).not.toHaveBeenCalled();
  expect(f.removeParent).not.toHaveBeenCalled();
  expect(f.removeWorkspace).not.toHaveBeenCalled();
});

it.each(['Stop refusal', 'parent changes'] as const)(
  'ends a blocked request after %s only explicitly, then requires new consent and keys',
  async (cause) => {
    const f = fixture();
    if (cause === 'Stop refusal') f.stop.mockRejectedValueOnce(new ApiError(409, 'Stop refused'));
    const view = render(<f.Actions />);
    await start();
    if (cause === 'parent changes') {
      f.state.sandbox = { ...f.state.sandbox, status: 'rotating' };
      await tick(1500);
    }
    const original = restoreDeletions('owner')[0]!;
    expect(original.stop?.phase).toBe('blocked');
    expect(screen.getByRole('button', { name: 'Check request' })).toBeDisabled();
    await close();
    await click('Delete original Agent');
    expect(restoreDeletions('owner')).toEqual([original]);
    await close();
    view.unmount();
    const resumed = render(<f.Actions />);
    await tick();
    expect(screen.getByRole('dialog')).toHaveAccessibleName('Deleting Agent');
    await click('End this request');
    await tick(1000);
    expect(restoreDeletions('owner')).toEqual([]);
    expect(f.stop).toHaveBeenCalledOnce();
    expect(f.remove).not.toHaveBeenCalled();
    expect(f.removeParent).not.toHaveBeenCalled();
    resumed.unmount();
    render(<f.Actions />);
    await tick();
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    f.state.sandbox = { ...f.state.sandbox, status: 'active' };
    await click('Delete original Agent');
    expect(screen.getByRole('checkbox')).not.toBeChecked();
    expect(screen.getByRole('button', { name: 'Stop and delete' })).toBeDisabled();
    fireEvent.click(screen.getByRole('checkbox'));
    await click('Stop and delete');
    const next = restoreDeletions('owner')[0]!;
    expect(next.key).not.toBe(original.key);
    expect(next.stop?.key).not.toBe(original.stop?.key);
    expect(f.stop).toHaveBeenCalledTimes(2);
    expect(f.remove).not.toHaveBeenCalled();
  },
);

it('invalidates a pending Stop before ending its blocked intent and resets keys in the same mounted owner', async () => {
  const f = fixture();
  let finish!: () => void;
  f.stop.mockImplementationOnce(
    () =>
      new Promise<void>((resolve) => {
        finish = resolve;
      }),
  );
  render(<f.Actions />);
  await start();
  f.state.sandbox = { ...f.state.sandbox, status: 'rotating' };
  await tick(1500);
  const original = restoreDeletions('owner')[0]!;
  expect(original.stop?.phase).toBe('blocked');
  await click('End this request');
  await tick(1000);
  expect(restoreDeletions('owner')).toEqual([]);
  f.state.sandbox = { ...f.state.sandbox, status: 'active' };
  f.state.status = 'failed';
  await click('Delete original Agent');
  expect(screen.getByRole('checkbox')).not.toBeChecked();
  expect(screen.getByRole('button', { name: 'Delete Agent' })).toBeDisabled();
  await act(async () => finish());
  await tick(3000);
  expect(restoreDeletions('owner')).toEqual([]);
  expect(f.remove).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('checkbox'));
  await click('Delete Agent');
  expect(f.remove).toHaveBeenCalledOnce();
  expect(f.remove.mock.calls[0]![1].idempotencyKey).not.toBe(original.key);
  expect(f.stop).toHaveBeenCalledOnce();
});

it.each(['ordinary unknown', 'Stop unknown', 'Delete accepted'] as const)(
  'ending blocked intent preserves another %s request and its original keys',
  async (phase) => {
    const f = fixture();
    const other: DeletionRequest = {
      action: { kind: 'delete_agent', agentId: 'agt_other' },
      key: 'other-original',
      accepted: phase === 'Delete accepted',
      stop:
        phase === 'ordinary unknown'
          ? undefined
          : {
              ...blocked.stop!,
              key: 'stop-other',
              phase: phase === 'Stop unknown' ? 'requested' : 'accepted',
            },
    };
    persistDeletion('owner', blocked);
    persistDeletion('owner', other);
    render(<f.Actions />);
    await tick();
    if (screen.queryByRole('button', { name: 'End this request' }) === null) {
      await close();
      await click('Delete original Agent');
    }
    await click('End this request');
    await tick(1000);
    expect(restoreDeletions('owner')).toEqual([other]);
    expect(f.stop).not.toHaveBeenCalled();
    expect(f.remove).not.toHaveBeenCalled();
    await click('Delete other Agent');
    expect(screen.queryByRole('button', { name: 'End this request' })).not.toBeInTheDocument();
    expect(restoreDeletions('owner')).toEqual([other]);
  },
);
