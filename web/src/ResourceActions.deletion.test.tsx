import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError } from '@cma/client';
import { useResourceActions } from './ResourceActions';
import { AGENT_DETAIL, agentClient } from './test/agent';
import { workspaceFixture } from './test/hierarchy';
import { persistDeletion, restoreDeletions } from './deletionRecovery';
import type { AgentDetail } from './api/types';
import type { CmaWebClient } from './api/client';

afterEach(() => {
  localStorage.clear();
  vi.useRealTimers();
  vi.restoreAllMocks();
});

function Actions({
  client,
  changed = vi.fn(),
  viewerId = 'owner',
}: {
  readonly client: CmaWebClient;
  readonly viewerId?: string;
  readonly changed?: (target: import('./agent/resourceChange').ResourceChange) => void;
}) {
  const actions = useResourceActions(client, changed, undefined, viewerId);
  return (
    <>
      {['first', 'second'].map((agentId) => (
        <button
          key={agentId}
          disabled={actions.pending}
          onClick={() => actions.select({ kind: 'delete_agent', agentId })}
        >
          Delete {agentId}
        </button>
      ))}
      <button
        disabled={actions.pending}
        onClick={() => actions.select({ kind: 'remove_workspace', workspaceId: 'wsp_1' })}
      >
        Delete workspace
      </button>
      <button disabled={actions.pending}>Unrelated action</button>
      {actions.notice}
      {actions.overlay}
    </>
  );
}

function client(overrides: Partial<CmaWebClient> = {}) {
  return agentClient({
    deleteAgent: async () => undefined,
    readAgent: async (id) => ({
      ...AGENT_DETAIL,
      id,
      status: 'stopped',
      worktree: { ...AGENT_DETAIL.worktree, agentId: id },
    }),
    readWorkspace: async () => workspaceFixture('stopped'),
    ...overrides,
  });
}

async function remove(name: string, target = 'Agent') {
  await act(async () => fireEvent.click(screen.getByRole('button', { name })));
  fireEvent.click(screen.getByRole('checkbox'));
  await act(async () => fireEvent.click(screen.getByRole('button', { name: `Delete ${target}` })));
}

it('backgrounds Agent and Workspace acceptance, preserving every status across reload without replay', async () => {
  const deleteAgent = vi.fn().mockResolvedValue(undefined);
  const removeWorkspace = vi.fn().mockResolvedValue(undefined);
  const readAgentEvents = vi.fn().mockResolvedValue({ items: [] });
  const api = client({ deleteAgent, removeWorkspace, readAgentEvents });
  const changed = vi.fn();
  const view = render(<Actions client={api} changed={changed} />);
  await remove('Delete first');
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Unrelated action' })).toBeEnabled();
  await remove('Delete second');
  await remove('Delete workspace', 'Workspace');
  expect(changed.mock.calls).toEqual([
    [{ kind: 'delete_agent', agentId: 'first' }],
    [{ kind: 'delete_agent', agentId: 'second' }],
    [{ kind: 'remove_workspace', workspaceId: 'wsp_1' }],
  ]);
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(restoreDeletions('owner')).toHaveLength(3);
  // No background history scan is needed just to leave cleanup running.
  expect(readAgentEvents).not.toHaveBeenCalled();
  const keys = deleteAgent.mock.calls.map(([, options]) => options.idempotencyKey);
  expect(new Set(keys).size).toBe(2);
  view.unmount();
  render(<Actions client={api} />);
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Unrelated action' })).toBeEnabled();
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Delete first' })));
  expect(await screen.findByRole('dialog', { name: 'Deleting Agent' })).toBeInTheDocument();
  expect(readAgentEvents).toHaveBeenCalledExactlyOnceWith('first', expect.anything());
  expect(deleteAgent).toHaveBeenCalledTimes(2);
  expect(removeWorkspace).toHaveBeenCalledTimes(1);
  expect(screen.queryByText('Agent deletion is confirmed.')).not.toBeInTheDocument();
});

it('keeps accepted status recoverable on access loss and removes only proven terminal cleanup', async () => {
  let missing = false;
  let complete = false;
  const api = client({
    readAgent: async (id) => {
      if (missing) throw new ApiError(404, 'Not found', 'not_found');
      return {
        ...AGENT_DETAIL,
        id,
        status: complete ? 'deleted' : 'stopped',
        worktree: { ...AGENT_DETAIL.worktree, agentId: id, status: complete ? 'removed' : 'ready' },
      };
    },
  });
  render(<Actions client={api} />);
  await remove('Delete first');
  await remove('Delete second');
  missing = true;
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Delete first' })));
  expect(await screen.findByRole('alert')).toHaveTextContent('could not be verified');
  expect(restoreDeletions('owner')).toHaveLength(2);
  await act(async () => fireEvent.click(screen.getByText('Close', { selector: 'button' })));
  missing = false;
  complete = true;
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Delete first' })));
  await screen.findByRole('dialog', { name: 'Agent deleted' });
  expect(restoreDeletions('owner').map((request) => request.action)).toEqual([
    { kind: 'delete_agent', agentId: 'second' },
  ]);
});

it('retains an uncertain original request across close and reload, requiring explicit retry', async () => {
  const deleteAgent = vi
    .fn()
    .mockRejectedValueOnce(new Error('Response lost'))
    .mockResolvedValue(undefined);
  const api = client({ deleteAgent });
  const view = render(<Actions client={api} />);
  await remove('Delete first');
  expect(await screen.findByRole('alert')).toHaveTextContent('Response lost');
  await act(async () => fireEvent.click(screen.getByText('Close', { selector: 'button' })));
  expect(screen.getByRole('button', { name: 'Unrelated action' })).toBeEnabled();
  view.unmount();
  render(<Actions client={api} />);
  await screen.findByRole('dialog', { name: 'Deleting Agent' });
  expect(deleteAgent).toHaveBeenCalledTimes(1);
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Check request' })));
  await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
  expect(deleteAgent.mock.calls[1]).toEqual(deleteAgent.mock.calls[0]);
  expect(restoreDeletions('owner')[0]?.accepted).toBe(true);
});

function animatePresence() {
  vi.useFakeTimers();
  const matchMedia = window.matchMedia.bind(window);
  vi.spyOn(window, 'matchMedia').mockImplementation((query) => ({
    ...matchMedia(query),
    matches: false,
  }));
}
const tick = (ms: number) => act(() => vi.advanceTimersByTimeAsync(ms));
const click = async (name: string) => {
  if (name.startsWith('View '))
    await act(async () => fireEvent.click(screen.getByRole('button', { name: 'View status' })));
  await act(async () => fireEvent.click(screen.getByRole('button', { name })));
};
const retained = (id = 'first') => ({
  action: { kind: 'delete_agent' as const, agentId: id },
  key: `original-${id}`,
  accepted: true,
});
const agentState = (complete = false): AgentDetail => ({
  ...AGENT_DETAIL,
  id: 'first',
  status: complete ? 'deleted' : 'stopped',
  worktree: { ...AGENT_DETAIL.worktree, agentId: 'first', status: complete ? 'removed' : 'ready' },
});

it.each([false, true])(
  'finishes the same deletion after a 100ms reopen, restored=%s',
  async (restored) => {
    animatePresence();
    const other = retained('second');
    persistDeletion('owner', other);
    if (restored) persistDeletion('owner', { ...retained(), accepted: false });
    let complete = false;
    const deleteAgent = vi.fn().mockResolvedValue(undefined);
    const readAgentEvents = vi.fn().mockResolvedValue({ items: [] });
    const api = client({
      deleteAgent,
      readAgentEvents,
      readAgent: async () => agentState(complete),
    });
    render(<Actions client={api} />);
    if (restored) await click('Check request');
    else await remove('Delete first');
    const original = restoreDeletions('owner').find(
      (request) => request.action.kind === 'delete_agent' && request.action.agentId === 'first',
    )!;
    expect(original.accepted).toBe(true);
    expect(readAgentEvents).not.toHaveBeenCalled();
    await tick(100);
    await click('View Agent deletion status (2)');
    complete = true;
    await tick(1500);
    expect(screen.getByRole('dialog', { name: 'Agent deleted' })).toBeVisible();
    expect(restoreDeletions('owner')).toEqual([other]);
    expect(deleteAgent).toHaveBeenCalledExactlyOnceWith('first', { idempotencyKey: original.key });
    if (restored) expect(original.key).toBe('original-first');
    expect(readAgentEvents).toHaveBeenCalledTimes(2);
  },
);

function heldReads() {
  const reads: { signal: AbortSignal; finish: (value: AgentDetail) => void }[] = [];
  const readAgent: CmaWebClient['readAgent'] = vi.fn(
    (_id, { signal } = {}) =>
      new Promise<AgentDetail>((finish) => {
        reads.push({ signal: signal!, finish });
      }),
  );
  return { reads, readAgent };
}

it('aborts each closed observer, ignores late reads and resumes only one poll after a hidden tab', async () => {
  animatePresence();
  const request = retained();
  persistDeletion('owner', request);
  const held = heldReads();
  const deleteAgent = vi.fn();
  const view = render(<Actions client={client({ readAgent: held.readAgent, deleteAgent })} />);
  await click('View Agent deletion status (1)');
  await tick(5000);
  expect(held.reads).toHaveLength(1);
  for (let index = 0; index < 3; index++) {
    await act(async () => fireEvent.click(screen.getByText('Close', { selector: 'button' })));
    expect(held.reads[index]!.signal.aborted).toBe(true);
    await tick(100);
    await click('View Agent deletion status (1)');
    expect(held.reads.filter((read) => !read.signal.aborted)).toHaveLength(1);
    await act(async () => held.reads[index]!.finish(agentState(true)));
    expect(screen.queryByRole('dialog', { name: 'Agent deleted' })).not.toBeInTheDocument();
    expect(restoreDeletions('owner')).toEqual([request]);
  }
  await act(async () => held.reads.at(-1)!.finish(agentState()));
  const hidden = vi.spyOn(document, 'hidden', 'get').mockReturnValue(true);
  await act(async () => document.dispatchEvent(new Event('visibilitychange')));
  await tick(5000);
  expect(held.reads).toHaveLength(4);
  hidden.mockReturnValue(false);
  await act(async () => {
    for (let index = 0; index < 3; index++) document.dispatchEvent(new Event('visibilitychange'));
  });
  expect(held.reads).toHaveLength(5);
  await act(async () => held.reads.at(-1)!.finish(agentState()));
  await tick(1500);
  expect(held.reads).toHaveLength(6);
  await act(async () => held.reads.at(-1)!.finish(agentState(true)));
  expect(screen.getByRole('dialog', { name: 'Agent deleted' })).toBeVisible();
  await tick(10000);
  expect(held.reads).toHaveLength(6);
  expect(restoreDeletions('owner')).toEqual([]);
  expect(deleteAgent).not.toHaveBeenCalled();
  view.unmount();
  expect(held.reads.every((read) => read.signal.aborted)).toBe(true);
});

it.each(['client', 'owner'] as const)(
  'replaces the observation on %s change and rejects late completion after unmount',
  async (change) => {
    animatePresence();
    const request = retained();
    persistDeletion('owner', request);
    persistDeletion('next-owner', request);
    const old = heldReads();
    const next = heldReads();
    const deleteAgent = vi.fn();
    const oldClient = client({ readAgent: old.readAgent, deleteAgent });
    const view = render(<Actions client={oldClient} />);
    await click('View Agent deletion status (1)');
    view.rerender(
      <Actions
        viewerId={change === 'owner' ? 'next-owner' : 'owner'}
        client={
          change === 'client' ? client({ readAgent: next.readAgent, deleteAgent }) : oldClient
        }
      />,
    );
    await act(async () => {});
    expect(old.reads[0]!.signal.aborted).toBe(true);
    const current = change === 'client' ? next.reads : old.reads.slice(1);
    expect(current).toHaveLength(1);
    await act(async () => old.reads[0]!.finish(agentState(true)));
    expect(screen.queryByRole('dialog', { name: 'Agent deleted' })).not.toBeInTheDocument();
    view.unmount();
    expect(current[0]!.signal.aborted).toBe(true);
    await act(async () => current[0]!.finish(agentState(true)));
    expect(restoreDeletions('owner')).toEqual([request]);
    expect(restoreDeletions('next-owner')).toEqual([request]);
    expect(deleteAgent).not.toHaveBeenCalled();
  },
);

it.each([401, 403, 404])(
  'retains accepted receipts after a rapid reopen receives %s',
  async (status) => {
    animatePresence();
    let missing = false;
    const deleteAgent = vi.fn().mockResolvedValue(undefined);
    render(
      <Actions
        client={client({
          deleteAgent,
          readAgent: async () => {
            if (missing) throw new ApiError(status, 'Unavailable', 'unavailable');
            return agentState();
          },
        })}
      />,
    );
    await remove('Delete first');
    const [original] = restoreDeletions('owner');
    await tick(100);
    missing = true;
    await click('View Agent deletion status (1)');
    expect(screen.getByRole('alert')).toHaveTextContent('could not be verified');
    expect(restoreDeletions('owner')).toEqual([original]);
    expect(screen.queryByRole('button', { name: 'Done' })).not.toBeInTheDocument();
    expect(deleteAgent).toHaveBeenCalledTimes(1);
  },
);
