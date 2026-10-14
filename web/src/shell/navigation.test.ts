import { act, renderHook } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { SandboxDetail } from '../api/types';
import { agentClient } from '../test/agent';
import { testClient } from '../test/client';
import { sandboxFixture, workspaceFixture } from '../test/hierarchy';
import { loadNavigation, useNavigation } from './navigation';

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe('navigation hierarchy loading', () => {
  it('detects a save beginning and finishing while Active without replacing the hierarchy during refresh', async () => {
    vi.useFakeTimers();
    const saving: SandboxDetail = {
      ...sandboxFixture(),
      lifecycle: {
        pause: { allowed: false, reason: 'checkpoint_in_progress' },
        revive: { allowed: false, reason: 'not_paused' },
        checkpoint: { phase: 'capture', startedAt: '2026-09-08T00:00:00Z', epoch: 7 },
      },
    };
    const readSandbox = vi
      .fn()
      .mockResolvedValueOnce(sandboxFixture())
      .mockResolvedValueOnce(saving)
      .mockResolvedValue(sandboxFixture());
    const client = agentClient({ readSandbox });
    const { result } = renderHook(() => useNavigation(client));
    await act(async () => {});
    expect(result.current).toMatchObject({
      status: 'ready',
      items: [{ sandbox: { lifecycle: { pause: { allowed: true } } } }],
    });
    await act(() => vi.advanceTimersByTimeAsync(5000));
    expect(result.current).toMatchObject({
      status: 'ready',
      items: [{ sandbox: { lifecycle: { checkpoint: { phase: 'capture' } } } }],
    });
    await act(() => vi.advanceTimersByTimeAsync(5000));
    expect(result.current).toMatchObject({
      status: 'ready',
      items: [{ sandbox: { lifecycle: { pause: { allowed: true } } } }],
    });
    expect(readSandbox).toHaveBeenCalledTimes(3);
    act(() => result.current.refresh());
    expect(result.current.status).toBe('ready');
    await act(async () => {});
  });

  it('keeps successful hierarchy data stale after failed refresh and clears the notice after recovery', async () => {
    vi.useFakeTimers();
    const readSandbox = vi
      .fn()
      .mockResolvedValueOnce(sandboxFixture())
      .mockRejectedValueOnce(new Error('unavailable'))
      .mockResolvedValue(sandboxFixture());
    const client = agentClient({ readSandbox });
    const { result } = renderHook(() => useNavigation(client));
    await act(async () => {});
    await act(() => vi.advanceTimersByTimeAsync(5000));
    expect(result.current).toMatchObject({
      status: 'ready',
      refreshError: expect.stringContaining('Reconnecting'),
      items: [{ sandbox: { id: 'sbx_1' }, workspaces: [{ agents: [{ id: 'agt_1' }] }] }],
    });
    await act(() => vi.advanceTimersByTimeAsync(5000));
    expect(result.current).not.toHaveProperty('refreshError');
  });

  it('never overlaps hierarchy reads and refreshes on visibility return, not while hidden or unmounted', async () => {
    vi.useFakeTimers();
    let resolveRead: (sandbox: SandboxDetail) => void = () => {};
    const readSandbox = vi
      .fn()
      .mockImplementationOnce(
        () =>
          new Promise<SandboxDetail>((resolve) => {
            resolveRead = resolve;
          }),
      )
      .mockResolvedValue(sandboxFixture());
    const client = agentClient({ readSandbox });
    const { unmount } = renderHook(() => useNavigation(client));
    await act(() => vi.advanceTimersByTimeAsync(20000));
    expect(readSandbox).toHaveBeenCalledTimes(1);
    await act(async () => {
      resolveRead(sandboxFixture());
    });
    const hidden = vi.spyOn(document, 'hidden', 'get').mockReturnValue(true);
    await act(() => vi.advanceTimersByTimeAsync(20000));
    expect(readSandbox).toHaveBeenCalledTimes(1);
    hidden.mockReturnValue(false);
    await act(async () => {
      document.dispatchEvent(new Event('visibilitychange'));
    });
    expect(readSandbox).toHaveBeenCalledTimes(2);
    unmount();
    await act(() => vi.advanceTimersByTimeAsync(20000));
    expect(readSandbox).toHaveBeenCalledTimes(2);
  });

  it('follows Sandbox pages and expands public Sandbox and Workspace detail projections', async () => {
    const sandbox = sandboxFixture();
    const workspace = workspaceFixture();
    const listSandboxes = vi
      .fn()
      .mockResolvedValueOnce({ items: [sandbox], next: 'next-page' })
      .mockResolvedValueOnce({ items: [] });
    const readSandbox = vi.fn().mockResolvedValue(sandbox);
    const readWorkspace = vi.fn().mockResolvedValue(workspace);
    const client = testClient({ listSandboxes, readSandbox, readWorkspace });

    await expect(loadNavigation(client, new AbortController().signal)).resolves.toEqual([
      { sandbox, workspaces: [workspace] },
    ]);
    expect(listSandboxes).toHaveBeenNthCalledWith(
      2,
      expect.objectContaining({ cursor: 'next-page' }),
    );
    expect(readSandbox).toHaveBeenCalledWith('sbx_1', expect.any(Object));
    expect(readWorkspace).toHaveBeenCalledWith('wsp_1', expect.any(Object));
  });

  it('rejects a non-advancing public page cursor', async () => {
    const listSandboxes = vi.fn().mockResolvedValue({ items: [], next: 'same' });
    await expect(
      loadNavigation(testClient({ listSandboxes }), new AbortController().signal),
    ).rejects.toThrow(/did not advance/i);
  });
});
