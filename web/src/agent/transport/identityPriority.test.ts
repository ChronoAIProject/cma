import { afterEach, expect, it, vi } from 'vitest';
import { createIdentityPriority } from './identityPriority';

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => (resolve = done));
  return { promise, resolve };
}
const flush = async () => {
  for (let index = 0; index < 12; index++) await Promise.resolve();
};
afterEach(() => vi.useRealTimers());

it('preserves the exact verifier and concurrent calls when disabled', async () => {
  const held = deferred<string>();
  const current = vi.fn(() => held.promise);
  const priority = createIdentityPriority(current);
  expect(priority.foreground).toBe(current);
  expect(priority.background).toBe(current);
  const calls = [priority.background(), priority.foreground()];
  expect(current).toHaveBeenCalledTimes(2);
  held.resolve('owner');
  await Promise.all(calls);
});

it('starts a distinct fresh verification for every queued caller', async () => {
  const first = deferred<string>();
  const current = vi.fn().mockReturnValueOnce(first.promise).mockResolvedValueOnce('changed');
  const priority = createIdentityPriority(current, true);
  const a = priority.foreground();
  const b = priority.foreground();
  await flush();
  expect(current).toHaveBeenCalledTimes(1);
  first.resolve('owner');
  await expect(a).resolves.toBe('owner');
  await expect(b).resolves.toBe('changed');
  expect(current).toHaveBeenCalledTimes(2);
});

it('limits the foreground burst while keeping FIFO order within a role', async () => {
  const probes = Array.from({ length: 4 }, () => deferred<string>());
  let next = 0;
  const current = vi.fn(() => probes[next++]!.promise);
  const priority = createIdentityPriority(current, true);
  const first = priority.foreground();
  const background = priority.background();
  const foreground1 = priority.foreground();
  const foreground2 = priority.foreground();
  await flush();
  probes[0]!.resolve('first');
  await flush();
  probes[1]!.resolve('foreground1');
  await flush();
  probes[2]!.resolve('background');
  await flush();
  probes[3]!.resolve('foreground2');
  expect(await Promise.all([first, background, foreground1, foreground2])).toEqual([
    'first',
    'background',
    'foreground1',
    'foreground2',
  ]);
});

it('serves an aged background job at the next available slot', async () => {
  vi.useFakeTimers();
  const held = deferred<string>();
  const current = vi
    .fn()
    .mockReturnValueOnce(held.promise)
    .mockResolvedValueOnce('aged')
    .mockResolvedValueOnce('foreground');
  const priority = createIdentityPriority(current, true);
  const first = priority.foreground();
  const aged = priority.background();
  const foreground = priority.foreground();
  await vi.advanceTimersByTimeAsync(501);
  held.resolve('first');
  expect(await Promise.all([first, aged, foreground])).toEqual(['first', 'aged', 'foreground']);
});

it('cancels queued checks without starting a verifier', async () => {
  const held = deferred<string>();
  const current = vi.fn(() => held.promise);
  const priority = createIdentityPriority(current, true);
  const first = priority.background();
  const abort = new AbortController();
  const reason = new Error('cancelled');
  const cancelled = priority.foreground(abort.signal).catch((error: unknown) => error);
  abort.abort(reason);
  expect(await cancelled).toBe(reason);
  held.resolve('owner');
  await first;
  await flush();
  expect(current).toHaveBeenCalledTimes(1);
});

it('settles active cancellation promptly but keeps a late probe inside its slot', async () => {
  const held = deferred<string>();
  const current = vi.fn().mockReturnValueOnce(held.promise).mockResolvedValueOnce('next');
  const priority = createIdentityPriority(current, true);
  const abort = new AbortController();
  const cancelled = priority.foreground(abort.signal).catch((error: unknown) => error);
  await flush();
  const later = priority.foreground();
  const reason = new Error('cancelled');
  abort.abort(reason);
  expect(await cancelled).toBe(reason);
  await flush();
  expect(current).toHaveBeenCalledTimes(1);
  expect(current.mock.calls[0]![0].aborted).toBe(true);
  held.resolve('late-success-must-not-authorize');
  await expect(later).resolves.toBe('next');
  expect(current).toHaveBeenCalledTimes(2);
});

it('bounds queued jobs and expires their wait without authorizing them', async () => {
  vi.useFakeTimers();
  const held = deferred<string>();
  const current = vi.fn(() => held.promise);
  const priority = createIdentityPriority(current, true);
  const active = priority.background();
  const queue = Array.from({ length: 32 }, () =>
    priority.foreground().catch((error: unknown) => error),
  );
  await expect(priority.foreground()).rejects.toThrow('queue is full');
  await vi.advanceTimersByTimeAsync(5_000);
  expect(
    (await Promise.all(queue)).every(
      (error) => error instanceof Error && error.message.includes('deadline'),
    ),
  ).toBe(true);
  expect(current).toHaveBeenCalledTimes(1);
  held.resolve('owner');
  await active;
  await flush();
  expect(vi.getTimerCount()).toBe(0);
});

it('propagates provider throttling and unauthenticated results without retry or cache', async () => {
  const throttled = Object.assign(new Error('provider busy'), {
    status: 429,
    retryAfterMs: 2300,
  });
  const current = vi.fn().mockRejectedValueOnce(throttled).mockResolvedValueOnce(undefined);
  const priority = createIdentityPriority(current, true);
  await expect(priority.foreground()).rejects.toBe(throttled);
  await expect(priority.foreground()).resolves.toBeUndefined();
  expect(current).toHaveBeenCalledTimes(2);
});

it('releases a failed synchronous probe and refuses already-aborted checks', async () => {
  const unavailable = new Error('503');
  const current = vi
    .fn<() => Promise<string>>()
    .mockImplementationOnce(() => {
      throw unavailable;
    })
    .mockResolvedValue('owner');
  const priority = createIdentityPriority(current, true);
  await expect(priority.foreground()).rejects.toBe(unavailable);
  const abort = new AbortController();
  abort.abort(unavailable);
  await expect(priority.foreground(abort.signal)).rejects.toBe(unavailable);
  await expect(priority.foreground()).resolves.toBe('owner');
  expect(current).toHaveBeenCalledTimes(2);
});

it('uses elapsed time for background aging even if the wall clock moves backwards', async () => {
  vi.useFakeTimers();
  const held = deferred<string>();
  const current = vi
    .fn()
    .mockReturnValueOnce(held.promise)
    .mockResolvedValueOnce('aged')
    .mockResolvedValueOnce('foreground');
  const priority = createIdentityPriority(current, true);
  const first = priority.foreground();
  const aged = priority.background();
  const foreground = priority.foreground();
  await vi.advanceTimersByTimeAsync(501);
  vi.setSystemTime(new Date(0));
  held.resolve('first');
  expect(await Promise.all([first, aged, foreground])).toEqual(['first', 'aged', 'foreground']);
});
