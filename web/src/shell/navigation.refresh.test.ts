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

it('keeps 11 Sandbox / 10 Workspace revival polling to three reads per tick, with bounded terminal rechecks', async () => {
  vi.useFakeTimers();
  const inventory = hierarchy();
  const client = hierarchyClient(() => inventory);
  const { result } = renderHook(() => useNavigation(client));
  await act(async () => {});
  expect(result.current).toMatchObject({ status: 'ready', items: inventory });
  expect(client.listSandboxes).toHaveBeenCalledTimes(1);
  expect(client.readSandbox).toHaveBeenCalledTimes(11);
  expect(client.readWorkspace).toHaveBeenCalledTimes(10);
  await act(() => vi.advanceTimersByTimeAsync(45000));
  expect(client.listSandboxes).toHaveBeenCalledTimes(31);
  expect(client.readSandbox).toHaveBeenCalledTimes(41);
  expect(client.readWorkspace).toHaveBeenCalledTimes(40);
  for (const item of inventory.slice(1)) {
    expect(
      vi.mocked(client.readSandbox).mock.calls.filter(([id]) => id === item.sandbox.id),
    ).toHaveLength(1);
  }
  await act(() => vi.advanceTimersByTimeAsync(15000));
  for (const item of inventory.slice(1)) {
    expect(
      vi.mocked(client.readSandbox).mock.calls.filter(([id]) => id === item.sandbox.id),
    ).toHaveLength(2);
  }
  expect(client.listSandboxes).toHaveBeenCalledTimes(41);
  expect(client.readSandbox).toHaveBeenCalledTimes(61);
  expect(client.readWorkspace).toHaveBeenCalledTimes(59);
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
    items: [
      { workspaces: [{ agents: [{ title: 'Fresh title', status: 'working' }] }] },
      ...inventory.slice(1),
    ],
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
  expect(result.current).toMatchObject({ status: 'ready', items: inventory });
  expect(client.readWorkspace).toHaveBeenCalledWith('wsp_new', expect.any(Object));
});

it('refreshes lifecycle eligibility at the settled and saving cadences while retaining retired cache', async () => {
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
    items: [
      { sandbox: { lifecycle: { checkpoint: { phase: 'capture' } } } },
      ...inventory.slice(1),
    ],
  });
  inventory[0] = {
    ...inventory[0]!,
    sandbox: { ...inventory[0]!.sandbox, lifecycle: sandboxFixture().lifecycle },
  };
  await act(() => vi.advanceTimersByTimeAsync(1500));
  expect(result.current).toMatchObject({
    status: 'ready',
    items: [{ sandbox: { lifecycle: { pause: { allowed: true } } } }, ...inventory.slice(1)],
  });
  expect(client.readSandbox).toHaveBeenCalledTimes(13);
});

it('drops no-longer-visible retired resources and never resurrects them from cache', async () => {
  vi.useFakeTimers();
  let inventory = hierarchy();
  const client = hierarchyClient(() => inventory);
  const { result } = renderHook(() => useNavigation(client));
  await act(async () => {});
  inventory = inventory.slice(0, 1);
  await act(() => vi.advanceTimersByTimeAsync(1500));
  expect(result.current).toMatchObject({ status: 'ready', items: inventory });
  expect(client.readSandbox).toHaveBeenCalledTimes(12);
});

it.each(['status', 'updatedAt', 'workspaceCount', 'name', 'profile', 'lifecycle'] as const)(
  'invalidates retired reuse when authorized summary %s changes',
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
    const result = await loadNavigation(client, new AbortController().signal, original);
    expect(result[1]).toEqual(inventory[1]);
    expect(client.readSandbox).toHaveBeenCalledTimes(2);
    expect(client.readWorkspace).toHaveBeenCalledTimes(2);
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
    expect(result.current).toMatchObject({ status: 'ready', items: inventory });
    expect(client.readSandbox).toHaveBeenCalledTimes(22);
    expect(client.readWorkspace).toHaveBeenCalledTimes(20);
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

it('forces revalidation on explicit refresh and never transfers the cache to a replacement client', async () => {
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
  expect(client.readSandbox).toHaveBeenCalledTimes(23);
  rerender({ client: replacement });
  await act(async () => {});
  expect(replacement.readSandbox).toHaveBeenCalledTimes(11);
  expect(replacement.readWorkspace).toHaveBeenCalledTimes(10);
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
