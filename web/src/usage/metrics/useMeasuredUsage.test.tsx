import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { useMeasuredUsage, usageSettingsChanged } from './useMeasuredUsage';

afterEach(() => vi.useRealTimers());
it('requires an effective true and returns to allocation after a failed refresh', async () => {
  const read = vi.fn().mockResolvedValueOnce(true).mockRejectedValue(new Error('Unavailable'));
  const { result } = renderHook(() => useMeasuredUsage(read));
  expect(result.current).toBe(false);
  await waitFor(() => expect(result.current).toBe(true));
  act(usageSettingsChanged);
  expect(result.current).toBe(false);
  await waitFor(() => expect(read).toHaveBeenCalledTimes(2));
  expect(result.current).toBe(false);
});
it.each([false, undefined, null, 'true'])(
  'keeps measured usage hidden for effective value %s',
  async (value) => {
    const read = vi.fn().mockResolvedValue(value);
    const { result } = renderHook(() => useMeasuredUsage(read));
    await waitFor(() => expect(read).toHaveBeenCalledTimes(1));
    expect(result.current).toBe(false);
  },
);
it('aborts superseded reads and ignores a late enabled response', async () => {
  let complete!: (value: boolean) => void;
  const read = vi
    .fn()
    .mockImplementationOnce(
      () =>
        new Promise<boolean>((resolve) => {
          complete = resolve;
        }),
    )
    .mockResolvedValue(false);
  const { result } = renderHook(() => useMeasuredUsage(read));
  const signal = read.mock.calls[0]![0].signal as AbortSignal;
  act(usageSettingsChanged);
  expect(signal.aborted).toBe(true);
  await act(async () => complete(true));
  expect(result.current).toBe(false);
});
it('reads only while open and visible and rereads on reopen', async () => {
  const read = vi.fn().mockResolvedValue(true);
  const { result, rerender } = renderHook(({ active }) => useMeasuredUsage(read, active), {
    initialProps: { active: false },
  });
  expect(read).not.toHaveBeenCalled();
  rerender({ active: true });
  await waitFor(() => expect(result.current).toBe(true));
  rerender({ active: false });
  expect(result.current).toBe(false);
  act(usageSettingsChanged);
  expect(read).toHaveBeenCalledTimes(1);
  rerender({ active: true });
  await waitFor(() => expect(read).toHaveBeenCalledTimes(2));
});
it('polls at most once per minute and coalesces in-flight periodic reads', async () => {
  vi.useFakeTimers();
  const read = vi.fn().mockResolvedValue(true);
  const { result, unmount } = renderHook(() => useMeasuredUsage(read));
  await act(async () => {
    await Promise.resolve();
  });
  expect(result.current).toBe(true);
  await act(async () => {
    vi.advanceTimersByTime(59000);
  });
  expect(read).toHaveBeenCalledTimes(1);
  read.mockImplementation(() => new Promise(() => {}));
  await act(async () => {
    vi.advanceTimersByTime(1000);
  });
  expect(read).toHaveBeenCalledTimes(2);
  await act(async () => {
    vi.advanceTimersByTime(120000);
  });
  expect(read).toHaveBeenCalledTimes(2);
  const signal = read.mock.calls[1]![0].signal as AbortSignal;
  unmount();
  expect(signal.aborted).toBe(true);
});
it('drops the previous principal setting when the reader changes', async () => {
  const first = vi.fn().mockResolvedValue(true);
  const second = vi.fn().mockResolvedValue(false);
  const { result, rerender } = renderHook(({ read }) => useMeasuredUsage(read), {
    initialProps: { read: first },
  });
  await waitFor(() => expect(result.current).toBe(true));
  rerender({ read: second });
  expect(result.current).toBe(false);
  await waitFor(() => expect(second).toHaveBeenCalledTimes(1));
});
