import { act, render, fireEvent, screen, renderHook } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { ApiError } from '@cma/client';
import {
  useDeletionNoticeReconciliation,
  reconcileDeletionRequests,
} from './useDeletionNoticeReconciliation';
import { persistDeletion, restoreDeletions } from './deletionRecovery';
import type { DeletionRequest } from './deletion';
import { useResourceActions } from './ResourceActions';
import type { CmaWebClient } from './api/client';
import { sandboxFixture } from './test/hierarchy';
import { AGENT_DETAIL, agentClient } from './test/agent';

const request: DeletionRequest = {
  action: { kind: 'delete_agent', agentId: AGENT_DETAIL.id },
  key: 'combined-delete',
  accepted: true,
  deleteSent: true,
  stop: {
    key: 'combined-stop',
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

it.each(['sandbox', 'workspace', 'worktree', 'worktreeSandbox', 'worktreeWorkspace'] as const)(
  'retains combined responsibility when a terminal read changes %s',
  async (field) => {
    persistDeletion('owner', request);
    const value = {
      ...terminal,
      sandbox: { ...terminal.sandbox, id: field === 'sandbox' ? 'other' : terminal.sandbox.id },
      workspace: {
        ...terminal.workspace,
        id: field === 'workspace' ? 'other' : terminal.workspace.id,
      },
      worktree: {
        ...terminal.worktree,
        id: field === 'worktree' ? 'other' : terminal.worktree.id,
        sandboxId: field === 'worktreeSandbox' ? 'other' : terminal.worktree.sandboxId,
        workspaceId: field === 'worktreeWorkspace' ? 'other' : terminal.worktree.workspaceId,
      },
    };
    const readAgent = vi.fn().mockResolvedValue(value);
    const source = agentClient({ readAgent });
    const view = renderHook(() => useDeletionNoticeReconciliation(source, 'owner', 0, true));
    await act(async () => {
      await view.result.current.client.readAgent(AGENT_DETAIL.id);
    });
    expect(restoreDeletions('owner')).toEqual([request]);
    expect(view.result.current.revision).toBe(0);
    readAgent.mockResolvedValue(terminal);
    await act(async () => {
      await view.result.current.client.readAgent(AGENT_DETAIL.id);
    });
    expect(restoreDeletions('owner')).toEqual([]);
    expect(view.result.current.revision).toBe(1);
  },
);

it.each(['key', 'sandboxId', 'workspaceId', 'worktreeId'] as const)(
  'does not settle a replacement with a changed Stop %s',
  async (field) => {
    persistDeletion('owner', request);
    let finish!: (value: typeof terminal) => void;
    const source = agentClient({
      readAgent: () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    });
    const view = renderHook(() => useDeletionNoticeReconciliation(source, 'owner', 0, true));
    const pending = view.result.current.client.readAgent(AGENT_DETAIL.id);
    await act(async () => {});
    const replacement: DeletionRequest = {
      ...request,
      stop: { ...request.stop!, [field]: 'replacement' },
    };
    persistDeletion('owner', replacement);
    await act(async () => {
      finish(terminal);
      await pending;
    });
    expect(restoreDeletions('owner')).toEqual([replacement]);
    expect(reconcileDeletionRequests([replacement], [replacement], [request])).toEqual([
      replacement,
    ]);
    expect(view.result.current.revision).toBe(0);
  },
);

it.each(['Agent preflight', 'parent preflight', 'Delete reply', 'Delete refusal'] as const)(
  'invalidates pending %s when an existing read settles late acceptance',
  async (stage) => {
    persistDeletion('owner', { ...request, accepted: false });
    const stopped = { ...AGENT_DETAIL, status: 'stopped' as const };
    const parent = { ...sandboxFixture(), owner: 'owner' };
    const readAgent = vi.fn().mockResolvedValue(stopped);
    const readSandbox = vi.fn().mockResolvedValue(parent);
    const stopAgent = vi.fn();
    const deleteAgent = vi.fn().mockResolvedValue(undefined);
    const source = agentClient({ readAgent, readSandbox, stopAgent, deleteAgent });
    let observedClient!: CmaWebClient;
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
        </>
      );
    }
    render(<Actions />);
    await act(async () => {});
    let finish!: () => void;
    const pending = new Promise<void>((resolve) => {
      finish = resolve;
    });
    if (stage === 'Agent preflight')
      readAgent.mockImplementationOnce(async () => {
        await pending;
        return stopped;
      });
    else if (stage === 'parent preflight')
      readSandbox.mockImplementationOnce(async () => {
        await pending;
        return parent;
      });
    else
      deleteAgent.mockImplementationOnce(async () => {
        await pending;
        if (stage === 'Delete refusal') throw new ApiError(409, 'Late refusal');
      });
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Check request' }));
    });
    persistDeletion('owner', request);
    const writes = vi.spyOn(localStorage, 'setItem');
    readAgent.mockResolvedValue(terminal);
    await act(async () => {
      await observedClient.readAgent(AGENT_DETAIL.id);
      expect(restoreDeletions('owner')).toEqual([]);
      writes.mockClear();
      finish();
    });
    expect(deleteAgent).toHaveBeenCalledTimes(stage.startsWith('Delete') ? 1 : 0);
    expect(stopAgent).not.toHaveBeenCalled();
    expect(writes).not.toHaveBeenCalled();
    writes.mockRestore();
    expect(restoreDeletions('owner')).toEqual([]);
    expect(screen.queryByRole('region', { name: 'Deletion request' })).not.toBeInTheDocument();
  },
);
