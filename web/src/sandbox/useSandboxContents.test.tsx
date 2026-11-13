import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { agentClient, AGENT_DETAIL } from '../test/agent';
import { sandboxFixture, workspaceFixture } from '../test/hierarchy';
import type { SandboxDetail, WorkspaceDetail } from '../api/types';
import { useSandboxContents } from './useSandboxContents';

beforeEach(() => vi.useFakeTimers());
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});
const tick = (ms = 0) => act(() => vi.advanceTimersByTimeAsync(ms));

function fixture(initial = sandboxFixture()) {
  const readWorkspace = vi.fn(async () => workspaceFixture());
  const readAgent = vi.fn(async () => AGENT_DETAIL);
  const client = agentClient({ readWorkspace, readAgent });
  const hook = renderHook(
    ({ sandbox, readEnabled }) => useSandboxContents({ client, sandbox, readEnabled }),
    { initialProps: { sandbox: initial, readEnabled: true } },
  );
  const rerender = (sandbox: SandboxDetail, readEnabled = true) =>
    hook.rerender({ sandbox, readEnabled });
  return { hook, rerender, readWorkspace, readAgent };
}

it('ignores new parent objects and refreshes child-only state within fifteen seconds', async () => {
  const sandbox = sandboxFixture();
  const f = fixture(sandbox);
  await tick();
  for (let i = 0; i < 20; i++) {
    f.rerender(structuredClone(sandbox));
    await tick(100);
  }
  expect(f.readWorkspace).toHaveBeenCalledTimes(1);
  expect(f.readAgent).toHaveBeenCalledTimes(1);
  // Sandbox hierarchy projection has the Workspace's status and counts, but
  // not its Agents' status, task state or title. None changes this parent key.
  const working = workspaceFixture('working');
  expect([working.status, working.agentCount, working.worktreeCount]).toEqual([
    sandbox.workspaces[0]!.status,
    sandbox.workspaces[0]!.agentCount,
    sandbox.workspaces[0]!.worktreeCount,
  ]);
  f.readWorkspace.mockResolvedValue(working);
  f.readAgent.mockResolvedValue({
    ...AGENT_DETAIL,
    status: 'working',
    title: 'Updated child title',
  });
  await tick(12_999);
  expect(f.readAgent).toHaveBeenCalledTimes(1);
  await tick(1);
  expect(f.readAgent).toHaveBeenCalledTimes(2);
  expect(f.hook.result.current.contents?.agents[0]).toMatchObject({
    status: 'working',
    title: 'Updated child title',
  });
});

it.each(['revision', 'status', 'agentCount', 'worktreeCount', 'membership'] as const)(
  'refreshes immediately when parent %s changes',
  async (change) => {
    const sandbox = sandboxFixture();
    const f = fixture(sandbox);
    await tick();
    const next: SandboxDetail = {
      ...sandbox,
      revision: change === 'revision' ? sandbox.revision + 1 : sandbox.revision,
      workspaces:
        change === 'membership'
          ? []
          : [
              {
                ...sandbox.workspaces[0]!,
                ...(change === 'status' ? { status: 'preparing' as const } : {}),
                ...(change === 'agentCount' ? { agentCount: 2 } : {}),
                ...(change === 'worktreeCount' ? { worktreeCount: 2 } : {}),
              },
            ],
    };
    f.rerender(next);
    await tick();
    if (change === 'membership') expect(f.hook.result.current.contents?.workspaces).toEqual([]);
    else expect(f.readWorkspace).toHaveBeenCalledTimes(2);
  },
);

it('bounds stable children for five minutes and extends to thirty seconds after ten unchanged minutes', async () => {
  const f = fixture();
  await tick(300_000);
  expect(f.readWorkspace).toHaveBeenCalledTimes(21);
  expect(f.readAgent).toHaveBeenCalledTimes(21);
  await tick(300_000);
  expect(f.readWorkspace).toHaveBeenCalledTimes(41);
  await tick(29_999);
  expect(f.readWorkspace).toHaveBeenCalledTimes(41);
  await tick(1);
  expect(f.readWorkspace).toHaveBeenCalledTimes(42);
  act(() => f.hook.result.current.retry());
  await tick();
  expect(f.readWorkspace).toHaveBeenCalledTimes(43);
  await tick(15_000);
  expect(f.readWorkspace).toHaveBeenCalledTimes(44);
});

it.each(['workspace', 'agent', 'identity'] as const)(
  'reports partial %s failure and retries in five seconds',
  async (failure) => {
    const f = fixture();
    await tick();
    if (failure === 'workspace') f.readWorkspace.mockRejectedValueOnce(new Error('offline'));
    else if (failure === 'identity')
      f.readWorkspace.mockResolvedValueOnce({ ...workspaceFixture(), sandboxId: 'sbx_other' });
    else f.readAgent.mockRejectedValueOnce(new Error('offline'));
    await tick(15_000);
    expect(f.hook.result.current.contents?.failed).toBe(true);
    await tick(4999);
    expect(f.readWorkspace).toHaveBeenCalledTimes(2);
    await tick(1);
    expect(f.readWorkspace).toHaveBeenCalledTimes(3);
    expect(f.hook.result.current.contents?.failed).toBe(false);
  },
);

it('suspends while hidden, resumes immediately and never overlaps a slow child read', async () => {
  const f = fixture();
  await tick();
  const hidden = vi.spyOn(document, 'hidden', 'get').mockReturnValue(true);
  act(() => document.dispatchEvent(new Event('visibilitychange')));
  await tick(60_000);
  expect(f.readWorkspace).toHaveBeenCalledTimes(1);
  let resolve!: (workspace: WorkspaceDetail) => void;
  f.readWorkspace.mockImplementationOnce(
    () =>
      new Promise((done) => {
        resolve = done;
      }),
  );
  hidden.mockReturnValue(false);
  act(() => document.dispatchEvent(new Event('visibilitychange')));
  await tick(60_000);
  expect(f.readWorkspace).toHaveBeenCalledTimes(2);
  act(() => document.dispatchEvent(new Event('visibilitychange')));
  expect(f.readWorkspace).toHaveBeenCalledTimes(2);
  await act(async () => resolve(workspaceFixture()));
  await tick(15_000);
  expect(f.readWorkspace).toHaveBeenCalledTimes(3);
  f.hook.unmount();
  await tick(60_000);
  expect(f.readWorkspace).toHaveBeenCalledTimes(3);
});
