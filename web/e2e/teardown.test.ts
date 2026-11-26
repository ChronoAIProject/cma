// @vitest-environment node
import type { TestInfo } from '@playwright/test';
import { afterEach, expect, it, vi } from 'vitest';
import { settlesWithin, teardownRemaining, teardownWindow } from './teardown';

afterEach(() => vi.useRealTimers());

it('reserves containment time inside the available allocation without resetting it', () => {
  for (const remaining of [0, 1, 100, 2_000, 30_000, 600_000]) {
    const { observationMs, releaseMs } = teardownWindow(remaining);
    expect(observationMs + releaseMs).toBeLessThanOrEqual(remaining);
    expect(observationMs).toBeLessThanOrEqual(5_000);
    expect(releaseMs).toBeLessThanOrEqual(2_000);
  }
  for (const remaining of [-1, NaN, Infinity])
    expect(teardownWindow(remaining)).toEqual({ observationMs: 0, releaseMs: 0 });
});

it('removes deadline timers on successful and rejected work', async () => {
  vi.useFakeTimers();
  expect(await settlesWithin(Promise.resolve(), 500)).toBe(true);
  await expect(settlesWithin(Promise.reject(Error('inspection failed')), 500)).rejects.toThrow(
    'inspection failed',
  );
  expect(vi.getTimerCount()).toBe(0);
});

it('keeps the deadline result after a late settlement and observes late rejection', async () => {
  vi.useFakeTimers();
  let reject!: (error: Error) => void;
  const work = new Promise<void>((_, fail) => {
    reject = fail;
  });
  const settlement = settlesWithin(work, 200);
  await vi.advanceTimersByTimeAsync(200);
  expect(await settlement).toBe(false);
  reject(Error('late unreadable body'));
  await Promise.resolve();
  expect(await settlement).toBe(false);
  expect(vi.getTimerCount()).toBe(0);
});

it('fails closed when the pinned runner deadline contract is unavailable', () => {
  for (const info of [
    {},
    { _timeoutManager: null },
    { _timeoutManager: { currentSlotDeadline: () => NaN } },
  ])
    expect(teardownRemaining(info as TestInfo)).toBe(0);
});
