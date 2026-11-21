import { act, renderHook } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { agentClient } from '../test/agent';
import { sandboxFixture } from '../test/hierarchy';
import type { SandboxDetail } from '../api/types';
import { useSandboxInput } from './sandboxInput';

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

const saving = (phase: 'capture' | 'resume_writers'): SandboxDetail => ({
  ...sandboxFixture(),
  lifecycle: {
    pause: { allowed: false, reason: 'checkpoint_in_progress' },
    revive: { allowed: false, reason: 'not_paused' },
    checkpoint: { phase, startedAt: '2026-09-08T00:00:00Z', epoch: 8 },
  },
});

it.each([
  ['active', 'runtime-one', 'live'],
  ['active', undefined, 'inactive'],
  ['active', ' ', 'inactive'],
  ['credential_rotating', 'runtime-one', 'live'],
  ['credential_rotating', undefined, 'inactive'],
  ['paused', undefined, 'paused'],
  ['paused', 'runtime-one', 'paused'],
  ['reviving', 'runtime-one', 'inactive'],
  ['pausing', 'runtime-one', 'inactive'],
] as const)(
  'requires a fresh active runtime for metrics: %s/%s',
  async (status, runtimeVersion, expected) => {
    const client = agentClient({
      readSandbox: async () => ({ ...sandboxFixture(status), runtimeVersion }),
    });
    const { result } = renderHook(() => useSandboxInput(client, 'agt_1', 'sbx_1'));
    expect(result.current.resourceAvailability.state).toBe('checking');
    await act(async () => {});
    expect(result.current.resourceAvailability.state).toBe(expected);
    if (status === 'paused') expect(result.current.inputAvailability.allowed).toBe(true);
  },
);

it('keeps live resource eligibility separate from checkpoint input holds and expires stale observations', async () => {
  vi.useFakeTimers();
  const readSandbox = vi
    .fn()
    .mockResolvedValueOnce(saving('capture'))
    .mockImplementation(() => new Promise(() => {}));
  const client = agentClient({ readSandbox });
  const { result } = renderHook(() => useSandboxInput(client, 'agt_1', 'sbx_1'));
  await act(async () => {});
  expect(result.current.inputAvailability.allowed).toBe(false);
  expect(result.current.resourceAvailability).toEqual({ state: 'live', runtimeVersion: '1.2.3' });
  await act(() => vi.advanceTimersByTimeAsync(11500));
  expect(result.current.resourceAvailability.state).toBe('unavailable');
  expect(readSandbox).toHaveBeenCalledTimes(2);
});

it('holds new input until a matching read, detects capture while Active and clears only after saving ends', async () => {
  vi.useFakeTimers();
  const readSandbox = vi
    .fn()
    .mockResolvedValueOnce(sandboxFixture())
    .mockResolvedValueOnce(saving('capture'))
    .mockResolvedValueOnce(saving('resume_writers'))
    .mockResolvedValue(sandboxFixture());
  const client = agentClient({ readSandbox });
  const { result, unmount } = renderHook(
    () => useSandboxInput(client, 'agt_1', 'sbx_1').inputAvailability,
  );
  expect(result.current.allowed).toBe(false);
  await act(async () => {});
  expect(result.current.allowed).toBe(true);
  await act(() => vi.advanceTimersByTimeAsync(5000));
  expect(result.current).toMatchObject({
    allowed: false,
    notice: expect.stringContaining('Saving progress'),
    savingStartedAt: '2026-09-08T00:00:00Z',
  });
  await act(() => vi.advanceTimersByTimeAsync(1500));
  expect(result.current).toMatchObject({
    allowed: false,
    notice: expect.stringContaining('Finishing save'),
  });
  await act(() => vi.advanceTimersByTimeAsync(1500));
  expect(result.current).toEqual({ allowed: true });
  expect(readSandbox).toHaveBeenCalledTimes(4);
  expect(readSandbox).toHaveBeenCalledWith(
    'sbx_1',
    expect.objectContaining({ signal: expect.any(AbortSignal) }),
  );
  unmount();
});

it.each(['missing', 'wrong_identity', 'failed'] as const)(
  'fails closed for %s lifecycle information and recovers on a later successful read',
  async (mode) => {
    vi.useFakeTimers();
    const readSandbox = vi.fn().mockResolvedValue(sandboxFixture());
    if (mode === 'missing')
      readSandbox.mockResolvedValueOnce({ ...sandboxFixture(), lifecycle: undefined });
    if (mode === 'wrong_identity')
      readSandbox.mockResolvedValueOnce({ ...sandboxFixture(), id: 'sbx_other' });
    if (mode === 'failed') readSandbox.mockRejectedValueOnce(new Error('read unavailable'));
    const client = agentClient({ readSandbox });
    const { result, unmount } = renderHook(
      () => useSandboxInput(client, 'agt_1', 'sbx_1').inputAvailability,
    );
    await act(async () => {});
    expect(result.current.allowed).toBe(false);
    expect(result.current.notice).toBeDefined();
    await act(() => vi.advanceTimersByTimeAsync(5000));
    expect(result.current).toEqual({ allowed: true });
    unmount();
  },
);

it('does not reuse Pause eligibility and preserves normal paused-message revival', async () => {
  const sandbox = sandboxFixture('paused');
  const client = agentClient({ readSandbox: async () => sandbox });
  const { result } = renderHook(() => useSandboxInput(client, 'agt_1', 'sbx_1').inputAvailability);
  await act(async () => {});
  expect(sandbox.lifecycle?.pause.allowed).toBe(false);
  expect(result.current.allowed).toBe(true);
});

it('retains the recorded save time on a failed refresh but holds input until a fresh response', async () => {
  vi.useFakeTimers();
  const client = agentClient({
    readSandbox: vi
      .fn()
      .mockResolvedValueOnce(saving('capture'))
      .mockRejectedValueOnce(new Error('offline'))
      .mockResolvedValue(sandboxFixture()),
  });
  const { result } = renderHook(() => useSandboxInput(client, 'agt_1', 'sbx_1').inputAvailability);
  await act(async () => {});
  await act(() => vi.advanceTimersByTimeAsync(1500));
  expect(result.current).toEqual({
    allowed: false,
    notice: 'Saving progress. Messages will be available when saving finishes.',
    savingStartedAt: '2026-09-08T00:00:00Z',
  });
  await act(() => vi.advanceTimersByTimeAsync(1500));
  expect(result.current).toEqual({ allowed: true });
});

it('aborts a stalled refresh, recovers on a fresh read and refuses the timed-out reply', async () => {
  vi.useFakeTimers();
  let late: ((sandbox: SandboxDetail) => void) | undefined;
  const readSandbox = vi
    .fn()
    .mockResolvedValueOnce(sandboxFixture())
    .mockImplementationOnce(
      () =>
        new Promise<SandboxDetail>((resolve) => {
          late = resolve;
        }),
    )
    .mockResolvedValue(sandboxFixture());
  const client = agentClient({ readSandbox });
  const { result, unmount } = renderHook(
    () => useSandboxInput(client, 'agt_1', 'sbx_1').inputAvailability,
  );
  await act(async () => {});
  expect(result.current.allowed).toBe(true);
  await act(() => vi.advanceTimersByTimeAsync(10000));
  // The next read keeps its full ten-second timeout before the observation expires.
  expect(result.current.allowed).toBe(true);
  expect(result.current.notice).toBeUndefined();
  await act(() => vi.advanceTimersByTimeAsync(4999));
  expect(readSandbox).toHaveBeenCalledTimes(2);
  const signal = readSandbox.mock.calls[1]?.[1].signal;
  expect(signal.aborted).toBe(false);
  await act(() => vi.advanceTimersByTimeAsync(1));
  expect(signal.aborted).toBe(true);
  expect(result.current.allowed).toBe(false);
  await act(() => vi.advanceTimersByTimeAsync(5000));
  expect(readSandbox).toHaveBeenCalledTimes(3);
  expect(result.current.allowed).toBe(true);
  await act(async () => late?.(saving('capture')));
  expect(result.current.allowed).toBe(true);
  unmount();
});

it('holds input after returning to a hidden tab until a new foreground read completes', async () => {
  vi.useFakeTimers();
  let hidden = false;
  vi.spyOn(document, 'hidden', 'get').mockImplementation(() => hidden);
  let resolveRead: (sandbox: SandboxDetail) => void = () => {};
  const readSandbox = vi
    .fn()
    .mockResolvedValueOnce(sandboxFixture())
    .mockImplementationOnce(
      () =>
        new Promise<SandboxDetail>((resolve) => {
          resolveRead = resolve;
        }),
    )
    .mockResolvedValue(sandboxFixture());
  const client = agentClient({ readSandbox });
  const { result, unmount } = renderHook(
    () => useSandboxInput(client, 'agt_1', 'sbx_1').inputAvailability,
  );
  await act(async () => {});
  await act(() => vi.advanceTimersByTimeAsync(5000));
  act(() => {
    hidden = true;
    document.dispatchEvent(new Event('visibilitychange'));
  });
  expect(result.current.allowed).toBe(true);
  await act(() => vi.advanceTimersByTimeAsync(20000));
  expect(result.current.allowed).toBe(false);
  expect(readSandbox).toHaveBeenCalledTimes(2);
  act(() => {
    hidden = false;
    document.dispatchEvent(new Event('visibilitychange'));
  });
  await act(async () => {
    resolveRead(sandboxFixture());
  });
  // The abandoned background read cannot keep the new foreground read blocked.
  expect(result.current.allowed).toBe(true);
  expect(readSandbox).toHaveBeenCalledTimes(3);
  unmount();
});

it('ignores late reads from another Agent and never overlaps or continues after unmount', async () => {
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
    .mockResolvedValue(saving('capture'));
  const client = agentClient({ readSandbox });
  const { result, rerender, unmount } = renderHook(
    ({ agentId }) => useSandboxInput(client, agentId, 'sbx_1').inputAvailability,
    { initialProps: { agentId: 'agt_1' } },
  );
  await act(() => vi.advanceTimersByTimeAsync(5000));
  expect(readSandbox).toHaveBeenCalledTimes(1);
  rerender({ agentId: 'agt_2' });
  await act(async () => {
    resolveRead(sandboxFixture());
  });
  expect(result.current.allowed).toBe(false);
  expect(result.current.notice).toContain('Saving progress');
  expect(readSandbox.mock.calls[0]?.[1].signal.aborted).toBe(true);
  unmount();
  await act(() => vi.advanceTimersByTimeAsync(20000));
  expect(readSandbox).toHaveBeenCalledTimes(2);
});
