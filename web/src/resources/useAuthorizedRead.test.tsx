import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { useAuthorizedRead, type AuthorizedValue } from './useAuthorizedRead';

afterEach(() => vi.useRealTimers());

it('clears previous values synchronously when the scope changes and discards late replies', async () => {
  let finish: ((value: AuthorizedValue<string>) => void) | undefined;
  const first = vi.fn(async () => ({ value: 'private first scope' }));
  const second = vi.fn(
    () =>
      new Promise<AuthorizedValue<string>>((resolve) => {
        finish = resolve;
      }),
  );
  const third = vi.fn(async () => ({ value: 'current scope' }));
  const hook = renderHook(({ read }) => useAuthorizedRead(read), {
    initialProps: { read: first as typeof second },
  });
  await waitFor(() =>
    expect(hook.result.current.state).toEqual({ status: 'ready', value: 'private first scope' }),
  );
  hook.rerender({ read: second });
  expect(hook.result.current.state.status).toBe('loading');
  hook.rerender({ read: third });
  await waitFor(() =>
    expect(hook.result.current.state).toEqual({ status: 'ready', value: 'current scope' }),
  );
  await act(async () => finish?.({ value: 'revoked late scope' }));
  expect(hook.result.current.state).toEqual({ status: 'ready', value: 'current scope' });
});

it('drops values at their authority deadline even when the refresh ignores cancellation', async () => {
  vi.useFakeTimers();
  const deadline = new Date(Date.now() + 10_000).toISOString();
  const read = vi
    .fn()
    .mockResolvedValueOnce({ value: 'shared secret', revalidateAt: deadline })
    .mockImplementation(() => new Promise(() => {}));
  const hook = renderHook(() => useAuthorizedRead(read));
  await act(async () => {});
  expect(hook.result.current.state.status).toBe('ready');
  await act(() => vi.advanceTimersByTimeAsync(10_000));
  expect(hook.result.current.state.status).toBe('loading');
  await act(() => vi.advanceTimersByTimeAsync(15_000));
  expect(hook.result.current.state.status).toBe('error');
  expect(JSON.stringify(hook.result.current.state)).not.toContain('shared secret');
});

it('refreshes ahead of a distinct authority deadline and withdraws the value at expiry', async () => {
  vi.useFakeTimers();
  const read = vi
    .fn()
    .mockResolvedValueOnce({
      value: 'current value',
      refreshAt: new Date(Date.now() + 2000).toISOString(),
      revalidateAt: new Date(Date.now() + 5000).toISOString(),
    })
    .mockImplementation(() => new Promise(() => {}));
  const hook = renderHook(() => useAuthorizedRead(read));
  await act(async () => {});
  await act(() => vi.advanceTimersByTimeAsync(2000));
  expect(read).toHaveBeenCalledTimes(2);
  expect(hook.result.current.state).toEqual({ status: 'ready', value: 'current value' });
  expect(hook.result.current.refreshing).toBe(true);
  await act(() => vi.advanceTimersByTimeAsync(3000));
  expect(hook.result.current.state.status).toBe('loading');
});

it('removes old rows on a failed refresh and aborts a request when disabled', async () => {
  vi.useFakeTimers();
  const signals: AbortSignal[] = [];
  const read = vi.fn(async (signal: AbortSignal) => {
    signals.push(signal);
    if (signals.length > 1) throw new Error('Access removed');
    return { value: 'old row' };
  });
  const hook = renderHook(({ enabled }) => useAuthorizedRead(enabled ? read : undefined), {
    initialProps: { enabled: true },
  });
  await act(async () => {});
  await act(() => vi.advanceTimersByTimeAsync(30_000));
  expect(hook.result.current.state).toEqual({ status: 'error', message: 'Access removed' });
  hook.rerender({ enabled: false });
  expect(hook.result.current.state.status).toBe('loading');
});

it('keeps a current value on foreground and coalesces focus and visibility into one refresh', async () => {
  let finish: ((value: AuthorizedValue<string>) => void) | undefined;
  const read = vi
    .fn()
    .mockResolvedValueOnce({ value: 'previous fact' })
    .mockImplementation(
      () =>
        new Promise<AuthorizedValue<string>>((resolve) => {
          finish = resolve;
        }),
    );
  const hook = renderHook(() => useAuthorizedRead(read));
  await waitFor(() => expect(hook.result.current.state.status).toBe('ready'));
  expect(hook.result.current.refreshing).toBe(false);
  act(() => {
    document.dispatchEvent(new Event('visibilitychange'));
    window.dispatchEvent(new Event('focus'));
  });
  // Still inside its authority deadline: no loading flash while revalidating.
  expect(hook.result.current.state).toEqual({ status: 'ready', value: 'previous fact' });
  expect(hook.result.current.refreshing).toBe(true);
  expect(read).toHaveBeenCalledTimes(2);
  await act(async () => finish?.({ value: 'current fact' }));
  expect(hook.result.current.state).toEqual({ status: 'ready', value: 'current fact' });
  expect(hook.result.current.refreshing).toBe(false);
});

it('clears retained data and refresh status when a foreground check refuses access', async () => {
  let refuse: ((reason: Error) => void) | undefined;
  const read = vi
    .fn()
    .mockResolvedValueOnce({ value: 'previous fact' })
    .mockImplementation(
      () =>
        new Promise<AuthorizedValue<string>>((_resolve, reject) => {
          refuse = reject;
        }),
    );
  const hook = renderHook(() => useAuthorizedRead(read));
  await waitFor(() => expect(hook.result.current.state.status).toBe('ready'));
  act(() => window.dispatchEvent(new Event('focus')));
  expect(hook.result.current.refreshing).toBe(true);
  await act(async () => refuse?.(new Error('Access removed')));
  expect(hook.result.current.state).toEqual({ status: 'error', message: 'Access removed' });
  expect(hook.result.current.refreshing).toBe(false);
});

it('withdraws a value on foreground when its deadline passed while timers were throttled', async () => {
  vi.useFakeTimers();
  const read = vi
    .fn()
    .mockResolvedValueOnce({
      value: 'shared secret',
      revalidateAt: new Date(Date.now() + 10_000).toISOString(),
    })
    .mockImplementation(() => new Promise(() => {}));
  const hook = renderHook(() => useAuthorizedRead(read));
  await act(async () => {});
  expect(hook.result.current.state.status).toBe('ready');
  // Move the wall clock past the deadline without running the expiration timer,
  // as a throttled background tab would.
  vi.setSystemTime(Date.now() + 11_000);
  act(() => window.dispatchEvent(new Event('focus')));
  expect(hook.result.current.state.status).toBe('loading');
  expect(JSON.stringify(hook.result.current.state)).not.toContain('shared secret');
});

it('does not refresh or clear when the tab becomes hidden', async () => {
  const read = vi.fn(async () => ({ value: 'fact' }));
  const visibility = vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('visible');
  const hook = renderHook(() => useAuthorizedRead(read));
  await waitFor(() => expect(hook.result.current.state.status).toBe('ready'));
  visibility.mockReturnValue('hidden');
  act(() => document.dispatchEvent(new Event('visibilitychange')));
  expect(hook.result.current.state).toEqual({ status: 'ready', value: 'fact' });
  expect(read).toHaveBeenCalledTimes(1);
  visibility.mockRestore();
});

it.each(['invalid timestamp', new Date(0).toISOString()])(
  'refuses expired or invalid authority: %s',
  async (revalidateAt) => {
    const read = vi.fn(async () => ({ value: 'unavailable', revalidateAt }));
    const hook = renderHook(() => useAuthorizedRead(read));
    await waitFor(() => expect(hook.result.current.state.status).toBe('error'));
  },
);
