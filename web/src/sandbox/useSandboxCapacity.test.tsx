import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { testClient } from '../test/client';
import { AVAILABLE_CAPACITY } from '../test/capacity';
import { useSandboxCapacity } from './useSandboxCapacity';

afterEach(() => vi.useRealTimers());

it('shares reads for one owner and never projects the previous owner during a switch', async () => {
  const read = vi
    .fn()
    .mockResolvedValueOnce(AVAILABLE_CAPACITY)
    .mockImplementation(() => new Promise(() => {}));
  const client = testClient({ readSandboxCapacity: read });
  const a = renderHook(({ owner }) => useSandboxCapacity(client, owner), {
    initialProps: { owner: 'first' },
  });
  const b = renderHook(() => useSandboxCapacity(client, 'first'));
  await waitFor(() => expect(a.result.current.value).toEqual(AVAILABLE_CAPACITY));
  expect(b.result.current.value).toEqual(AVAILABLE_CAPACITY);
  expect(read).toHaveBeenCalledTimes(1);
  a.rerender({ owner: 'second' });
  expect(a.result.current.value).toBeUndefined();
  await waitFor(() => expect(read).toHaveBeenCalledTimes(2));
  expect(b.result.current.value).toEqual(AVAILABLE_CAPACITY);
});

it('times out unavailable reads and preserves an explicit stale observation until retry succeeds', async () => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  const read = vi
    .fn()
    .mockResolvedValueOnce(AVAILABLE_CAPACITY)
    .mockImplementationOnce(() => new Promise(() => {}))
    .mockResolvedValue(AVAILABLE_CAPACITY);
  const client = testClient({ readSandboxCapacity: read });
  const hook = renderHook(() => useSandboxCapacity(client, 'owner'));
  await waitFor(() => expect(hook.result.current.value).toEqual(AVAILABLE_CAPACITY));
  await act(async () => {
    window.dispatchEvent(new Event('focus'));
    await vi.advanceTimersByTimeAsync(5_000);
  });
  expect(hook.result.current.error).toBe('Sandbox capacity could not be checked');
  expect(hook.result.current.value).toEqual(AVAILABLE_CAPACITY);
  await act(() => hook.result.current.refresh());
  expect(hook.result.current.error).toBeUndefined();
});
