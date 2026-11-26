import { act, renderHook } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { ApiError } from '@cma/client';
import { useDeletionNoticeReconciliation } from './useDeletionNoticeReconciliation';
import { forgetDeletion, persistDeletion, restoreDeletions } from './deletionRecovery';
import type { DeletionRequest } from './deletion';
import { AGENT_DETAIL, agentClient } from './test/agent';
import { sandboxFixture, workspaceFixture } from './test/hierarchy';

const request: DeletionRequest = {
  key: 'synthetic-accepted-delete',
  accepted: true,
  action: { kind: 'delete_sandbox', sandboxId: 'sbx_1' },
};

it('settles only the accepted request proved terminal by an existing read', async () => {
  persistDeletion('owner', request);
  const other = {
    ...request,
    key: 'other',
    action: { kind: 'delete_sandbox' as const, sandboxId: 'other' },
  };
  persistDeletion('owner', other);
  persistDeletion('another-owner', request);
  const readSandbox = vi.fn().mockResolvedValue(sandboxFixture('retiring'));
  const client = agentClient({ readSandbox });
  const view = renderHook(() => useDeletionNoticeReconciliation(client, 'owner', 0, true));
  expect(readSandbox).not.toHaveBeenCalled();
  await act(async () => {
    await view.result.current.client.readSandbox('sbx_1');
  });
  expect(restoreDeletions('owner')).toEqual([request, other]);
  readSandbox.mockResolvedValue(sandboxFixture('retired'));
  await act(async () => {
    await view.result.current.client.readSandbox('sbx_1');
  });
  expect(restoreDeletions('owner')).toEqual([other]);
  expect(restoreDeletions('another-owner')).toEqual([request]);
  expect(view.result.current.revision).toBe(1);
  expect(view.result.current.settlements).toEqual({
    client: view.result.current.client,
    viewerId: 'owner',
    requests: [request],
  });
  expect(readSandbox).toHaveBeenCalledTimes(2);
});

it.each(['unknown', 'ambiguous', 'wrong-resource'] as const)(
  'retains %s responsibility after terminal detail',
  async (mode) => {
    persistDeletion('owner', { ...request, accepted: mode !== 'unknown' });
    if (mode === 'ambiguous') persistDeletion('owner', { ...request, key: 'second-key' });
    const before = restoreDeletions('owner');
    const client = agentClient({
      readSandbox: async () => ({
        ...sandboxFixture('retired'),
        id: mode === 'wrong-resource' ? 'other' : 'sbx_1',
      }),
    });
    const view = renderHook(() => useDeletionNoticeReconciliation(client, 'owner', 0, true));
    await act(async () => {
      await view.result.current.client.readSandbox('sbx_1');
    });
    expect(restoreDeletions('owner')).toEqual(before);
    expect(view.result.current.revision).toBe(0);
  },
);

it.each([403, 404, 503])('does not turn a %s read failure into completion', async (status) => {
  persistDeletion('owner', request);
  const failure = new ApiError(status, 'Synthetic read failure');
  const client = agentClient({ readSandbox: vi.fn().mockRejectedValue(failure) });
  const view = renderHook(() => useDeletionNoticeReconciliation(client, 'owner', 0, true));
  await expect(view.result.current.client.readSandbox('sbx_1')).rejects.toBe(failure);
  expect(restoreDeletions('owner')).toEqual([request]);
});

it.each([
  'owner',
  'client',
  'epoch',
  'uncertain',
  'unmount',
  'abort',
  'replacement',
  'late-acceptance',
] as const)('rejects a terminal response across %s changes', async (change) => {
  persistDeletion('owner', { ...request, accepted: change !== 'late-acceptance' });
  let finish!: (value: ReturnType<typeof sandboxFixture>) => void;
  const client = agentClient({
    readSandbox: () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  });
  const view = renderHook(
    ({ source, owner, epoch, authorized }) =>
      useDeletionNoticeReconciliation(source, owner, epoch, authorized),
    {
      initialProps: { source: client, owner: 'owner', epoch: 0, authorized: true },
    },
  );
  const controller = new AbortController();
  const read = view.result.current.client
    .readSandbox('sbx_1', { signal: controller.signal })
    .catch(() => undefined);
  await act(async () => {});
  if (change === 'unmount') view.unmount();
  else if (change === 'abort') controller.abort();
  else if (change === 'replacement') {
    forgetDeletion('owner', request.key);
    persistDeletion('owner', { ...request, key: 'replacement-key' });
  } else if (change === 'late-acceptance') persistDeletion('owner', request);
  else
    view.rerender({
      source: change === 'client' ? agentClient() : client,
      owner: change === 'owner' ? 'another-owner' : 'owner',
      epoch: change === 'epoch' ? 1 : 0,
      authorized: change !== 'uncertain',
    });
  await act(async () => {
    finish(sandboxFixture('retired'));
    await read;
  });
  expect(restoreDeletions('owner')).toHaveLength(1);
  expect(restoreDeletions('owner')[0]?.key).toBe(
    change === 'replacement' ? 'replacement-key' : request.key,
  );
});

it('requires removed Agent worktree and exact Agent association', async () => {
  const agentRequest: DeletionRequest = {
    ...request,
    action: { kind: 'delete_agent', agentId: 'agt_1' },
  };
  persistDeletion('owner', agentRequest);
  const readAgent = vi.fn().mockResolvedValue({ ...AGENT_DETAIL, status: 'deleted' });
  const client = agentClient({ readAgent });
  const view = renderHook(() => useDeletionNoticeReconciliation(client, 'owner', 0, true));
  await act(async () => {
    await view.result.current.client.readAgent('agt_1');
  });
  expect(restoreDeletions('owner')).toEqual([agentRequest]);
  readAgent.mockResolvedValue({
    ...AGENT_DETAIL,
    status: 'deleted',
    worktree: { ...AGENT_DETAIL.worktree, status: 'removed', agentId: 'other' },
  });
  await act(async () => {
    await view.result.current.client.readAgent('agt_1');
  });
  expect(restoreDeletions('owner')).toEqual([agentRequest]);
  readAgent.mockResolvedValue({
    ...AGENT_DETAIL,
    status: 'deleted',
    worktree: { ...AGENT_DETAIL.worktree, status: 'removed' },
  });
  await act(async () => {
    await view.result.current.client.readAgent('agt_1');
  });
  expect(restoreDeletions('owner')).toEqual([]);
});

it('settles a removed Workspace without clearing its parent request', async () => {
  persistDeletion('owner', request);
  persistDeletion('owner', {
    ...request,
    key: 'workspace-key',
    action: { kind: 'remove_workspace', workspaceId: 'wsp_1' },
  });
  const client = agentClient({
    readWorkspace: async () => ({ ...workspaceFixture(), status: 'removed' }),
  });
  const view = renderHook(() => useDeletionNoticeReconciliation(client, 'owner', 0, true));
  await act(async () => {
    await view.result.current.client.readWorkspace('wsp_1');
  });
  expect(restoreDeletions('owner')).toEqual([request]);
});

it('keeps the client and retained-view identity stable across authentication rechecks', () => {
  const client = agentClient();
  const view = renderHook(
    ({ epoch, authorized }) => useDeletionNoticeReconciliation(client, 'owner', epoch, authorized),
    {
      initialProps: { epoch: 0, authorized: true },
    },
  );
  const original = view.result.current.client;
  view.rerender({ epoch: 1, authorized: false });
  expect(view.result.current.client).toBe(original);
  view.rerender({ epoch: 2, authorized: true });
  expect(view.result.current.client).toBe(original);
});

const retiredDetail = {
  ...AGENT_DETAIL,
  sandbox: { ...AGENT_DETAIL.sandbox, status: 'retired' as const },
};

it('uses one existing Agent read to show the selected retired row and settle only its parent', async () => {
  const sibling = {
    ...request,
    key: 'sibling',
    action: { kind: 'delete_sandbox' as const, sandboxId: 'sbx_2' },
  };
  const unknown = {
    ...request,
    key: 'unknown',
    accepted: false,
    action: { kind: 'delete_agent' as const, agentId: 'agt_1' },
  };
  for (const item of [request, sibling, unknown]) persistDeletion('owner', item);
  const readAgent = vi.fn().mockResolvedValue(retiredDetail);
  const client = agentClient({ readAgent });
  const view = renderHook(() => useDeletionNoticeReconciliation(client, 'owner', 3, true, 'agt_1'));
  expect(readAgent).not.toHaveBeenCalled();
  expect(view.result.current.selectedRetired).toBeUndefined();
  await act(async () => {
    await view.result.current.client.readAgent('agt_1');
  });
  expect(view.result.current.selectedRetired).toEqual({
    detail: retiredDetail,
    viewerId: 'owner',
    authEpoch: 3,
  });
  expect(restoreDeletions('owner')).toEqual([sibling, unknown]);
  expect(view.result.current.settlements?.requests).toEqual([request]);
  expect(readAgent).toHaveBeenCalledOnce();
});

it.each(['retiring', 'wrong-agent', 'wrong-worktree', 'wrong-workspace', 'wrong-parent'] as const)(
  'refuses the retired projection and parent settlement for %s',
  async (mode) => {
    persistDeletion('owner', request);
    const detail = {
      ...retiredDetail,
      id: mode === 'wrong-agent' ? 'other' : retiredDetail.id,
      sandbox: {
        ...retiredDetail.sandbox,
        status: mode === 'retiring' ? ('retiring' as const) : ('retired' as const),
      },
      worktree: {
        ...retiredDetail.worktree,
        agentId: mode === 'wrong-worktree' ? 'other' : 'agt_1',
        workspaceId: mode === 'wrong-workspace' ? 'other' : 'wsp_1',
        sandboxId: mode === 'wrong-parent' ? 'other' : 'sbx_1',
      },
    };
    const client = agentClient({ readAgent: async () => detail });
    const view = renderHook(() =>
      useDeletionNoticeReconciliation(client, 'owner', 0, true, 'agt_1'),
    );
    await act(async () => {
      await view.result.current.client.readAgent('agt_1');
    });
    expect(view.result.current.selectedRetired).toBeUndefined();
    expect(restoreDeletions('owner')).toEqual([request]);
  },
);

it.each(['unknown', 'ambiguous', 'replacement', 'late-acceptance'] as const)(
  'keeps %s parent responsibility even when the retired detail is current',
  async (mode) => {
    persistDeletion('owner', {
      ...request,
      accepted: !['unknown', 'late-acceptance'].includes(mode),
    });
    if (mode === 'ambiguous') persistDeletion('owner', { ...request, key: 'second-key' });
    let finish!: (value: typeof retiredDetail) => void;
    const client = agentClient({
      readAgent: () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    });
    const view = renderHook(() =>
      useDeletionNoticeReconciliation(client, 'owner', 0, true, 'agt_1'),
    );
    const read = view.result.current.client.readAgent('agt_1');
    await act(async () => {});
    if (mode === 'replacement') {
      forgetDeletion('owner', request.key);
      persistDeletion('owner', { ...request, key: 'replacement' });
    } else if (mode === 'late-acceptance') persistDeletion('owner', request);
    const before = restoreDeletions('owner');
    await act(async () => {
      finish(retiredDetail);
      await read;
    });
    expect(view.result.current.selectedRetired?.detail).toEqual(retiredDetail);
    expect(restoreDeletions('owner')).toEqual(before);
    expect(view.result.current.revision).toBe(0);
  },
);

it.each(['owner', 'client', 'epoch', 'uncertain', 'route', 'unmount', 'abort'] as const)(
  'discards late selected-retired detail across %s changes',
  async (mode) => {
    persistDeletion('owner', request);
    let finish!: (value: typeof retiredDetail) => void;
    const client = agentClient({
      readAgent: () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    });
    const initialProps = {
      source: client,
      owner: 'owner',
      epoch: 0,
      authorized: true,
      agentId: 'agt_1',
    };
    const view = renderHook(
      ({ source, owner, epoch, authorized, agentId }) =>
        useDeletionNoticeReconciliation(source, owner, epoch, authorized, agentId),
      { initialProps },
    );
    const controller = new AbortController();
    const read = view.result.current.client
      .readAgent('agt_1', { signal: controller.signal })
      .catch(() => undefined);
    await act(async () => {});
    if (mode === 'unmount') view.unmount();
    else if (mode === 'abort') controller.abort();
    else
      view.rerender({
        source: mode === 'client' ? agentClient() : client,
        owner: mode === 'owner' ? 'another-owner' : 'owner',
        epoch: mode === 'epoch' ? 1 : 0,
        authorized: mode !== 'uncertain',
        agentId: mode === 'route' ? 'agt_2' : 'agt_1',
      });
    await act(async () => {
      finish(retiredDetail);
      await read;
    });
    expect(view.result.current.selectedRetired).toBeUndefined();
    expect(restoreDeletions('owner')).toEqual([request]);
  },
);

it('does not revive a prior selected-retired witness when returning without a fresh read', async () => {
  const readAgent = vi.fn().mockResolvedValue(retiredDetail);
  const client = agentClient({ readAgent });
  const view = renderHook(
    ({ agentId }) => useDeletionNoticeReconciliation(client, 'owner', 0, true, agentId),
    {
      initialProps: { agentId: 'agt_1' },
    },
  );
  await act(async () => {
    await view.result.current.client.readAgent('agt_1');
  });
  expect(view.result.current.selectedRetired?.detail).toEqual(retiredDetail);
  view.rerender({ agentId: 'agt_2' });
  view.rerender({ agentId: 'agt_1' });
  expect(view.result.current.selectedRetired).toBeUndefined();
  expect(readAgent).toHaveBeenCalledOnce();
});

it.each([403, 404, 503])(
  'withdraws selected-retired detail on a fresh %s read failure',
  async (status) => {
    const readAgent = vi
      .fn()
      .mockResolvedValueOnce(retiredDetail)
      .mockRejectedValueOnce(new ApiError(status, 'Read unavailable'));
    const client = agentClient({ readAgent });
    const view = renderHook(() =>
      useDeletionNoticeReconciliation(client, 'owner', 0, true, 'agt_1'),
    );
    await act(async () => {
      await view.result.current.client.readAgent('agt_1');
    });
    expect(view.result.current.selectedRetired?.detail).toEqual(retiredDetail);
    await act(async () => {
      await expect(view.result.current.client.readAgent('agt_1')).rejects.toBeInstanceOf(ApiError);
    });
    expect(view.result.current.selectedRetired).toBeUndefined();
  },
);
