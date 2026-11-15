import { act, renderHook } from '@testing-library/react';
import { ApiError } from '@chronoai/cma-sdk';
import { afterEach, expect, it, vi } from 'vitest';
import { agentClient } from '../test/agent';
import { sandboxFixture } from '../test/hierarchy';
import { useSandboxInput } from './sandboxInput';

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

it.each([3000, 5200, 7000])(
  'keeps five minutes of successful %i ms refreshes sendable',
  async (delayMs) => {
    vi.useFakeTimers();
    vi.spyOn(document, 'hidden', 'get').mockReturnValue(false);
    const read = vi.fn(
      () =>
        new Promise<ReturnType<typeof sandboxFixture>>((resolve) => {
          setTimeout(() => resolve(sandboxFixture()), delayMs);
        }),
    );
    const client = agentClient({ readSandbox: read });
    const hook = renderHook(() => useSandboxInput(client, 'agt_1', 'sbx_1'));
    await act(() => vi.advanceTimersByTimeAsync(delayMs));
    const end = performance.now() + 300_000;
    setTimeout(() => undefined, 300_000);
    while (performance.now() < end) {
      expect(hook.result.current.inputAvailability).toEqual({ allowed: true });
      expect(hook.result.current.resourceAvailability.state).toBe('live');
      await act(() => vi.advanceTimersToNextTimerAsync());
    }
    expect(hook.result.current.inputAvailability).toEqual({ allowed: true });
    // Same completion-based cadence as the baseline: the grace adds no reads.
    expect(read).toHaveBeenCalledTimes(delayMs === 3000 ? 38 : delayMs === 5200 ? 30 : 26);
    hook.unmount();
  },
);

it.each([3000, 30_000])(
  'revalidates after %i ms hidden according to the observation age',
  async (hiddenMs) => {
    vi.useFakeTimers();
    let hidden = false;
    vi.spyOn(document, 'hidden', 'get').mockImplementation(() => hidden);
    const read = vi.fn(
      () =>
        new Promise<ReturnType<typeof sandboxFixture>>((resolve) => {
          setTimeout(() => resolve(sandboxFixture()), 3000);
        }),
    );
    const client = agentClient({ readSandbox: read });
    const hook = renderHook(() => useSandboxInput(client, 'agt_1', 'sbx_1'));
    await act(() => vi.advanceTimersByTimeAsync(3000));
    act(() => {
      hidden = true;
      document.dispatchEvent(new Event('visibilitychange'));
    });
    await act(() => vi.advanceTimersByTimeAsync(hiddenMs));
    expect(read).toHaveBeenCalledTimes(1);
    act(() => {
      hidden = false;
      document.dispatchEvent(new Event('visibilitychange'));
    });
    expect(read).toHaveBeenCalledTimes(2);
    expect(hook.result.current.inputAvailability.allowed).toBe(hiddenMs === 3000);
    if (hiddenMs === 30_000) {
      expect(hook.result.current.inputAvailability.notice).toBe('Checking Sandbox status.');
      expect(hook.result.current.resourceAvailability.state).toBe('checking');
    }
    await act(() => vi.advanceTimersByTimeAsync(3000));
    expect(hook.result.current.inputAvailability).toEqual({ allowed: true });
    hook.unmount();
  },
);

it('retains one transient failure, holds on the second and resets the count after success', async () => {
  vi.useFakeTimers();
  const read = vi.fn(async () => sandboxFixture());
  const client = agentClient({ readSandbox: read });
  const hook = renderHook(() => useSandboxInput(client, 'agt_1', 'sbx_1'));
  await act(() => vi.advanceTimersByTimeAsync(0));
  read.mockRejectedValueOnce(new ApiError(503, 'Temporary failure'));
  await act(() => vi.advanceTimersByTimeAsync(5000));
  expect(hook.result.current.inputAvailability.allowed).toBe(true);
  read.mockRejectedValueOnce(new ApiError(503, 'Temporary failure'));
  await act(() => vi.advanceTimersByTimeAsync(5000));
  expect(hook.result.current.inputAvailability.notice).toContain('Reconnecting');
  expect(hook.result.current.inputAvailability.allowed).toBe(false);
  await act(() => vi.advanceTimersByTimeAsync(5000));
  read.mockRejectedValueOnce(new ApiError(503, 'Temporary failure'));
  await act(() => vi.advanceTimersByTimeAsync(5000));
  expect(hook.result.current.inputAvailability.allowed).toBe(true);
  hook.unmount();
});

it.each([401, 403, 404, 'identity'] as const)(
  'immediately withdraws retained authority on %s',
  async (failure) => {
    vi.useFakeTimers();
    const read = vi.fn(async () => sandboxFixture());
    const client = agentClient({ readSandbox: read });
    const hook = renderHook(() => useSandboxInput(client, 'agt_1', 'sbx_1'));
    await act(() => vi.advanceTimersByTimeAsync(0));
    if (failure === 'identity')
      read.mockResolvedValueOnce({ ...sandboxFixture(), id: 'sbx_other' });
    else read.mockRejectedValueOnce(new ApiError(failure, 'Refused'));
    await act(() => vi.advanceTimersByTimeAsync(5000));
    expect(hook.result.current.inputAvailability.allowed).toBe(false);
    expect(hook.result.current.progressObservation).toBeUndefined();
    hook.unmount();
  },
);

it('checks the monotonic deadline on return even if background timers were suspended', async () => {
  vi.useFakeTimers();
  let hidden = false;
  vi.spyOn(document, 'hidden', 'get').mockImplementation(() => hidden);
  const read = vi.fn(async () => sandboxFixture());
  const client = agentClient({ readSandbox: read });
  const hook = renderHook(() => useSandboxInput(client, 'agt_1', 'sbx_1'));
  await act(() => vi.advanceTimersByTimeAsync(0));
  act(() => {
    hidden = true;
    document.dispatchEvent(new Event('visibilitychange'));
  });
  read.mockImplementation(() => new Promise(() => {}));
  vi.spyOn(performance, 'now').mockReturnValue(performance.now() + 30_000);
  act(() => {
    hidden = false;
    document.dispatchEvent(new Event('visibilitychange'));
  });
  expect(hook.result.current.inputAvailability.allowed).toBe(false);
  expect(hook.result.current.inputAvailability.notice).toBe('Checking Sandbox status.');
  hook.unmount();
});
