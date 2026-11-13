import { act, renderHook } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { agentClient } from '../test/agent';
import { sandboxFixture } from '../test/hierarchy';
import { useSandboxInput } from './sandboxInput';

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

it('backs off unchanged readiness with jitter, resets on progress and keeps the lifecycle freshness fence', async () => {
  vi.useFakeTimers();
  vi.spyOn(Math, 'random').mockReturnValue(0.5);
  let sandbox = sandboxFixture('reviving');
  const read = vi.fn(async () => sandbox);
  const client = agentClient({ readSandbox: read });
  const hook = renderHook(() => useSandboxInput(client, 'agt_1', 'sbx_1'));
  await act(() => vi.advanceTimersByTimeAsync(30000));
  expect(read.mock.calls.length).toBeLessThanOrEqual(8);
  expect(hook.result.current.inputAvailability.allowed).toBe(false);
  sandbox = sandboxFixture();
  await act(() => vi.advanceTimersByTimeAsync(5500));
  expect(hook.result.current.inputAvailability.allowed).toBe(true);
  const count = read.mock.calls.length;
  // Once ready, only the separate steady lifecycle fence remains, never the
  // fast readiness cadence. It still catches checkpoints/revocation promptly.
  await act(() => vi.advanceTimersByTimeAsync(10000));
  expect(read).toHaveBeenCalledTimes(count + 2);
  read.mockImplementation(() => new Promise(() => {}));
  await act(() => vi.advanceTimersByTimeAsync(15000));
  expect(hook.result.current.inputAvailability.allowed).toBe(false);
  expect(hook.result.current.resourceAvailability.state).toBe('unavailable');
  hook.unmount();
});

it('does not refetch Sandbox detail for rerenders or a focus storm', async () => {
  vi.useFakeTimers();
  const read = vi.fn(async () => sandboxFixture());
  const client = agentClient({ readSandbox: read });
  const hook = renderHook(() => useSandboxInput(client, 'agt_1', 'sbx_1'));
  await act(() => vi.advanceTimersByTimeAsync(0));
  for (let i = 0; i < 50; i++) {
    hook.rerender();
    act(() => window.dispatchEvent(new Event('focus')));
  }
  expect(read).toHaveBeenCalledTimes(1);
  hook.unmount();
});
