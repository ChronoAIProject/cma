import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError } from '@chronoai/cma-sdk';
import { useResourceActions } from './ResourceActions';
import { AGENT_DETAIL, agentClient } from './test/agent';
import { workspaceFixture } from './test/hierarchy';
import { restoreDeletions } from './deletionRecovery';
import type { CmaWebClient } from './api/client';

afterEach(() => localStorage.clear());

function Actions({ client }: { readonly client: CmaWebClient }) {
  const actions = useResourceActions(client, vi.fn(), undefined, 'owner');
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
  fireEvent.click(await screen.findByRole('checkbox'));
  await act(async () => fireEvent.click(screen.getByRole('button', { name: `Delete ${target}` })));
}

it('backgrounds Agent and Workspace acceptance, preserving every status across reload without replay', async () => {
  const deleteAgent = vi.fn().mockResolvedValue(undefined);
  const removeWorkspace = vi.fn().mockResolvedValue(undefined);
  const readAgentEvents = vi.fn().mockResolvedValue({ items: [] });
  const api = client({ deleteAgent, removeWorkspace, readAgentEvents });
  const view = render(<Actions client={api} />);
  await remove('Delete first');
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Unrelated action' })).toBeEnabled();
  await remove('Delete second');
  await remove('Delete workspace', 'Workspace');
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
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Refresh status' })));
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
