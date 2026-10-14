import { act, renderHook } from '@testing-library/react';
import { ApiError } from '@chronoai/cma-sdk';
import { afterEach, expect, it, vi } from 'vitest';
import type { CmaWebClient } from '../api/client';
import { testClient } from '../test/client';
import { sandboxFixture, workspaceFixture } from '../test/hierarchy';
import { loadNavigation, useNavigation, type NavigationSandbox } from './navigation';

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

function hierarchy(): NavigationSandbox[] {
  return Array.from({ length: 11 }, (_, index) => {
    const sandboxId = `sbx_${String(index)}`;
    const workspaceId = `wsp_${String(index)}`;
    const workspace = workspaceFixture();
    const workspaces =
      index === 10
        ? []
        : [
            {
              ...workspace,
              id: workspaceId,
              sandboxId,
              agents: workspace.agents.map((agent) => ({
                ...agent,
                id: `agt_${String(index)}`,
                sandboxId,
                workspaceId,
                worktreeId: `wkt_${String(index)}`,
              })),
              worktrees: workspace.worktrees.map((worktree) => ({
                ...worktree,
                id: `wkt_${String(index)}`,
                sandboxId,
                workspaceId,
                agentId: `agt_${String(index)}`,
              })),
            },
          ];
    return {
      sandbox: {
        ...sandboxFixture(index === 0 ? 'reviving' : 'retired'),
        id: sandboxId,
        workspaces,
        workspaceCount: workspaces.length,
        worktreeCount: workspaces.length,
        agentCount: workspaces.length,
      },
      workspaces,
    };
  });
}

function hierarchyClient(read: () => readonly NavigationSandbox[]): CmaWebClient {
  return testClient({
    listSandboxes: vi.fn(async () => ({ items: read().map(({ sandbox }) => sandbox) })),
    readSandbox: vi.fn(async (id) => read().find(({ sandbox }) => sandbox.id === id)!.sandbox),
    readWorkspace: vi.fn(async (id) =>
      read()
        .flatMap(({ workspaces }) => workspaces)
        .find((workspace) => workspace.id === id)!,
    ),
  });
}

it('keeps revival polling to three reads per tick without loading deleted subtrees, including initial load and minute boundaries', async () => {
  vi.useFakeTimers();
  const inventory = hierarchy();
  const client = hierarchyClient(() => inventory);
  const { result } = renderHook(() => useNavigation(client));
  await act(async () => {});
  expect(result.current).toMatchObject({ status: 'ready', items: inventory.slice(0, 1) });
  expect(client.listSandboxes).toHaveBeenCalledTimes(1);
  expect(client.readSandbox).toHaveBeenCalledTimes(1);
  expect(client.readWorkspace).toHaveBeenCalledTimes(1);
  await act(() => vi.advanceTimersByTimeAsync(45000));
  expect(client.listSandboxes).toHaveBeenCalledTimes(31);
  expect(client.readSandbox).toHaveBeenCalledTimes(31);
  expect(client.readWorkspace).toHaveBeenCalledTimes(31);
  for (const item of inventory.slice(1)) {
    expect(
      vi.mocked(client.readSandbox).mock.calls.filter(([id]) => id === item.sandbox.id),
    ).toHaveLength(0);
  }
  await act(() => vi.advanceTimersByTimeAsync(15000));
  for (const item of inventory.slice(1)) {
    expect(
      vi.mocked(client.readSandbox).mock.calls.filter(([id]) => id === item.sandbox.id),
    ).toHaveLength(0);
  }
  expect(client.listSandboxes).toHaveBeenCalledTimes(41);
  expect(client.readSandbox).toHaveBeenCalledTimes(41);
  expect(client.readWorkspace).toHaveBeenCalledTimes(41);
});

it('rereads mutable descendants when summary counts stay unchanged and discovers new Workspaces and Agents', async () => {
  vi.useFakeTimers();
  const inventory = hierarchy();
  const client = hierarchyClient(() => inventory);
  const { result } = renderHook(() => useNavigation(client));
  await act(async () => {});
  const first = inventory[0]!;
  const workspace = first.workspaces[0]!;
  const changed = {
    ...workspace,
    agents: workspace.agents.map((agent) => ({
      ...agent,
      title: 'Fresh title',
      status: 'working' as const,
    })),
  };
  inventory[0] = { sandbox: first.sandbox, workspaces: [changed] };
  await act(() => vi.advanceTimersByTimeAsync(1500));
  expect(result.current).toMatchObject({
    status: 'ready',
    items: [{ workspaces: [{ agents: [{ title: 'Fresh title', status: 'working' }] }] }],
  });
  const added = {
    ...changed,
    id: 'wsp_new',
    agents: [
      { ...changed.agents[0]!, id: 'agt_new', workspaceId: 'wsp_new', worktreeId: 'wkt_new' },
    ],
    worktrees: [
      { ...changed.worktrees[0]!, id: 'wkt_new', workspaceId: 'wsp_new', agentId: 'agt_new' },
    ],
  };
  inventory[0] = {
    sandbox: {
      ...first.sandbox,
      workspaceCount: 2,
      agentCount: 2,
      worktreeCount: 2,
      workspaces: [changed, added],
    },
    workspaces: [changed, added],
  };
  await act(() => vi.advanceTimersByTimeAsync(1500));
  expect(result.current).toMatchObject({ status: 'ready', items: inventory.slice(0, 1) });
  expect(client.readWorkspace).toHaveBeenCalledWith('wsp_new', expect.any(Object));
});

it('refreshes lifecycle eligibility at the settled and saving cadences without reading deleted resources', async () => {
  vi.useFakeTimers();
  const inventory = hierarchy();
  inventory[0] = {
    ...inventory[0]!,
    sandbox: {
      ...inventory[0]!.sandbox,
      ...sandboxFixture(),
      id: 'sbx_0',
      workspaces: inventory[0]!.workspaces,
    },
  };
  const client = hierarchyClient(() => inventory);
  const { result } = renderHook(() => useNavigation(client));
  await act(async () => {});
  inventory[0] = {
    ...inventory[0]!,
    sandbox: {
      ...inventory[0]!.sandbox,
      lifecycle: {
        pause: { allowed: false, reason: 'checkpoint_in_progress' },
        revive: { allowed: false, reason: 'not_paused' },
        checkpoint: { phase: 'capture', startedAt: '2026-09-08T00:00:00Z', epoch: 1 },
      },
    },
  };
  await act(() => vi.advanceTimersByTimeAsync(5000));
  expect(result.current).toMatchObject({
    status: 'ready',
    items: [{ sandbox: { lifecycle: { checkpoint: { phase: 'capture' } } } }],
  });
  inventory[0] = {
    ...inventory[0]!,
    sandbox: { ...inventory[0]!.sandbox, lifecycle: sandboxFixture().lifecycle },
  };
  await act(() => vi.advanceTimersByTimeAsync(1500));
  expect(result.current).toMatchObject({
    status: 'ready',
    items: [{ sandbox: { lifecycle: { pause: { allowed: true } } } }],
  });
  expect(client.readSandbox).toHaveBeenCalledTimes(3);
});

it('drops a newly deleted Sandbox and its descendants without another detail read', async () => {
  vi.useFakeTimers();
  let inventory = hierarchy();
  const client = hierarchyClient(() => inventory);
  const { result } = renderHook(() => useNavigation(client));
  await act(async () => {});
  inventory = inventory.map((item) => ({
    ...item,
    sandbox: { ...item.sandbox, status: 'retired' },
  }));
  await act(() => vi.advanceTimersByTimeAsync(1500));
  expect(result.current).toMatchObject({ status: 'ready', items: [] });
  expect(client.readSandbox).toHaveBeenCalledTimes(1);
});

it('does not load descendants if deletion completes between the list and detail reads', async () => {
  const inventory = hierarchy();
  const client = hierarchyClient(() => inventory);
  vi.mocked(client.readSandbox).mockResolvedValueOnce({
    ...inventory[0]!.sandbox,
    status: 'retired',
  });
  await expect(loadNavigation(client, new AbortController().signal)).resolves.toEqual([]);
  expect(client.readWorkspace).not.toHaveBeenCalled();
});

it.each(['status', 'updatedAt', 'workspaceCount', 'name', 'profile', 'lifecycle'] as const)(
  'keeps deleted resources out of navigation when summary %s changes, but reads nonterminal resources',
  async (field) => {
    const original = hierarchy();
    const inventory = structuredClone(original);
    const changed = inventory[1]!;
    const changes = {
      status: 'retiring' as const,
      updatedAt: '2026-09-08T00:00:00Z',
      workspaceCount: 2,
      name: 'Renamed',
      profile: { ...changed.sandbox.profile, revision: 9 },
      lifecycle: {
        ...changed.sandbox.lifecycle!,
        pause: { allowed: false, reason: 'work_in_progress' as const },
      },
    };
    inventory[1] = { ...changed, sandbox: { ...changed.sandbox, [field]: changes[field] } };
    const client = hierarchyClient(() => inventory);
    const result = await loadNavigation(client, new AbortController().signal);
    expect(result).toEqual(
      field === 'status'
        ? [inventory[0], { sandbox: inventory[1]!.sandbox, workspaces: [] }]
        : inventory.slice(0, 1),
    );
    expect(client.readSandbox).toHaveBeenCalledTimes(field === 'status' ? 2 : 1);
    expect(client.readWorkspace).toHaveBeenCalledTimes(1);
  },
);

it.each([401, 403, 404])(
  'clears previously visible hierarchy on public denial %s and reauthorizes details on recovery',
  async (status) => {
    vi.useFakeTimers();
    const inventory = hierarchy();
    const client = hierarchyClient(() => inventory);
    const { result } = renderHook(() => useNavigation(client));
    await act(async () => {});
    vi.mocked(client.listSandboxes).mockRejectedValueOnce(new ApiError(status, 'Denied'));
    await act(() => vi.advanceTimersByTimeAsync(1500));
    expect(result.current).toMatchObject({ status: 'error', message: 'Denied' });
    expect(result.current).not.toHaveProperty('items');
    await act(() => vi.advanceTimersByTimeAsync(5000));
    expect(result.current).toMatchObject({ status: 'ready', items: inventory.slice(0, 1) });
    expect(client.readSandbox).toHaveBeenCalledTimes(2);
    expect(client.readWorkspace).toHaveBeenCalledTimes(2);
  },
);

it.each(['readSandbox', 'readWorkspace'] as const)(
  'fails closed on %s visibility denial during refresh',
  async (method) => {
    vi.useFakeTimers();
    const inventory = hierarchy();
    const client = hierarchyClient(() => inventory);
    const { result } = renderHook(() => useNavigation(client));
    await act(async () => {});
    vi.mocked(client[method]).mockRejectedValueOnce(new ApiError(403, 'Denied'));
    await act(() => vi.advanceTimersByTimeAsync(1500));
    expect(result.current).toMatchObject({ status: 'error', message: 'Denied' });
    expect(result.current).not.toHaveProperty('items');
  },
);

it('revalidates the visible hierarchy on explicit refresh and with a replacement client', async () => {
  vi.useFakeTimers();
  const inventory = hierarchy();
  const client = hierarchyClient(() => inventory);
  const replacement = hierarchyClient(() => inventory);
  const { result, rerender } = renderHook(({ client }) => useNavigation(client), {
    initialProps: { client },
  });
  await act(async () => {});
  await act(() => vi.advanceTimersByTimeAsync(1500));
  act(() => result.current.refresh());
  await act(async () => {});
  expect(client.readSandbox).toHaveBeenCalledTimes(3);
  rerender({ client: replacement });
  await act(async () => {});
  expect(replacement.readSandbox).toHaveBeenCalledTimes(1);
  expect(replacement.readWorkspace).toHaveBeenCalledTimes(1);
});

it.each(['sandbox', 'workspace'] as const)(
  'rejects mismatched %s ancestry before publishing a hierarchy',
  async (kind) => {
    const inventory = hierarchy();
    const client = hierarchyClient(() => inventory);
    if (kind === 'sandbox')
      vi.mocked(client.readSandbox).mockResolvedValueOnce({
        ...inventory[0]!.sandbox,
        id: 'sbx_wrong',
      });
    else
      vi.mocked(client.readWorkspace).mockResolvedValueOnce({
        ...inventory[0]!.workspaces[0]!,
        sandboxId: 'sbx_wrong',
      });
    await expect(loadNavigation(client, new AbortController().signal)).rejects.toThrow(
      'hierarchy referred to different resources.',
    );
  },
);
