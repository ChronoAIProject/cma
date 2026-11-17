import { act, fireEvent, renderHook } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError } from '@cma/client';
import { useAuthorizedRead } from './useAuthorizedRead';

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});
it('preserves a 429 deadline through reload, focus storms and hidden tabs, then recovers', async () => {
  vi.useFakeTimers();
  vi.spyOn(Math, 'random').mockReturnValue(0.5);
  const visibility = vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('visible');
  const read = vi
    .fn()
    .mockRejectedValueOnce(new ApiError(429, 'Busy', undefined, undefined, 20_000))
    .mockResolvedValue({ value: 'current authority' });
  const hook = renderHook(() => useAuthorizedRead(read));
  await act(async () => {});
  expect(hook.result.current.state).toMatchObject({
    status: 'error',
    transient: true,
    retryAt: Date.now() + 20_500,
  });
  for (let i = 0; i < 5; i++) {
    act(() => {
      hook.result.current.reload();
      fireEvent.focus(window);
    });
  }
  await act(() => vi.advanceTimersByTimeAsync(20_499));
  expect(read).toHaveBeenCalledTimes(1);
  visibility.mockReturnValue('hidden');
  fireEvent(document, new Event('visibilitychange'));
  await act(() => vi.advanceTimersByTimeAsync(60_000));
  expect(read).toHaveBeenCalledTimes(1);
  visibility.mockReturnValue('visible');
  act(() => {
    fireEvent(document, new Event('visibilitychange'));
    fireEvent.focus(window);
  });
  await act(async () => {});
  expect(read).toHaveBeenCalledTimes(2);
  expect(hook.result.current.state).toEqual({ status: 'ready', value: 'current authority' });
  hook.unmount();
  expect(vi.getTimerCount()).toBe(0);
});
it('refuses a permanent authorization error without automatic retry', async () => {
  vi.useFakeTimers();
  const read = vi.fn().mockRejectedValue(new ApiError(403, 'Forbidden'));
  const hook = renderHook(() => useAuthorizedRead(read));
  await act(async () => {});
  expect(hook.result.current.state).toEqual({ status: 'error', message: 'Forbidden' });
  await act(() => vi.advanceTimersByTimeAsync(29_999));
  expect(read).toHaveBeenCalledTimes(1);
  hook.unmount();
});
