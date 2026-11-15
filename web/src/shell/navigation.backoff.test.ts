import { act, renderHook } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { testClient } from '../test/client';
import { sandboxFixture, workspaceFixture } from '../test/hierarchy';
import { useNavigation } from './navigation';

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

function setup() {
  const sandbox = sandboxFixture();
  const workspace = workspaceFixture();
  const client = testClient({
    listSandboxes: vi.fn(async () => ({ items: [sandbox] })),
    readSandbox: vi.fn(async () => sandbox),
    readWorkspace: vi.fn(async () => workspace),
  });
  return { client, sandbox, workspace };
}

it('caps unchanged hierarchy polling at sixty seconds and resets after changes and explicit refresh', async () => {
  vi.useFakeTimers();
  const { client, workspace } = setup();
  const { result } = renderHook(() => useNavigation(client));
  await act(async () => {});
  for (const [index, delay] of [15000, 30000, 60000, 60000].entries()) {
    await act(() => vi.advanceTimersByTimeAsync(delay - 1));
    expect(client.listSandboxes).toHaveBeenCalledTimes(index + 1);
    await act(() => vi.advanceTimersByTimeAsync(1));
    expect(client.listSandboxes).toHaveBeenCalledTimes(index + 2);
  }
  vi.mocked(client.readWorkspace).mockResolvedValue({
    ...workspace,
    agents: workspace.agents.map((agent) => ({ ...agent, title: 'Changed elsewhere' })),
  });
  await act(() => vi.advanceTimersByTimeAsync(60000));
  expect(client.listSandboxes).toHaveBeenCalledTimes(6);
  await act(() => vi.advanceTimersByTimeAsync(15000));
  expect(client.listSandboxes).toHaveBeenCalledTimes(7);
  await act(async () => result.current.refresh());
  expect(client.listSandboxes).toHaveBeenCalledTimes(8);
  await act(() => vi.advanceTimersByTimeAsync(15000));
  expect(client.listSandboxes).toHaveBeenCalledTimes(9);
});

it('retains the hierarchy on transient failures and restores the base retry interval', async () => {
  vi.useFakeTimers();
  const { client } = setup();
  const { result } = renderHook(() => useNavigation(client));
  await act(async () => {});
  await act(() => vi.advanceTimersByTimeAsync(45000));
  vi.mocked(client.listSandboxes).mockRejectedValueOnce(new Error('unavailable'));
  await act(() => vi.advanceTimersByTimeAsync(60000));
  expect(result.current).toMatchObject({
    status: 'ready',
    refreshError: expect.stringContaining('Reconnecting'),
  });
  await act(() => vi.advanceTimersByTimeAsync(15000));
  expect(client.listSandboxes).toHaveBeenCalledTimes(5);
  expect(result.current).not.toHaveProperty('refreshError');
});

it('resets after visibility changes while an aborted read is still settling', async () => {
  vi.useFakeTimers();
  const { client, sandbox } = setup();
  let finish!: (value: { items: (typeof sandbox)[] }) => void;
  vi.mocked(client.listSandboxes).mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const hidden = vi.spyOn(document, 'hidden', 'get').mockReturnValue(false);
  renderHook(() => useNavigation(client));
  await act(async () => {
    hidden.mockReturnValue(true);
    document.dispatchEvent(new Event('visibilitychange'));
    hidden.mockReturnValue(false);
    document.dispatchEvent(new Event('visibilitychange'));
    finish({ items: [sandbox] });
  });
  await act(() => vi.advanceTimersByTimeAsync(0));
  expect(client.listSandboxes).toHaveBeenCalledTimes(2);
  expect(client.readSandbox).toHaveBeenCalledOnce();
});
