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

it('clears cached facts on foreground before obtaining new authority', async () => {
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
  act(() => window.dispatchEvent(new Event('focus')));
  expect(hook.result.current.state.status).toBe('loading');
  await act(async () => finish?.({ value: 'current fact' }));
  expect(hook.result.current.state).toEqual({ status: 'ready', value: 'current fact' });
});

it.each(['invalid timestamp', new Date(0).toISOString()])(
  'refuses expired or invalid authority: %s',
  async (revalidateAt) => {
    const read = vi.fn(async () => ({ value: 'unavailable', revalidateAt }));
    const hook = renderHook(() => useAuthorizedRead(read));
    await waitFor(() => expect(hook.result.current.state.status).toBe('error'));
  },
);
