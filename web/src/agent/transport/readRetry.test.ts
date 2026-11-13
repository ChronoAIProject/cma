import { afterEach, expect, it, vi } from 'vitest';
import { ApiError, retryAfterMilliseconds } from '@chronoai/cma-sdk';
import { readRetryDelay } from './readRetry';

afterEach(() => vi.restoreAllMocks());
it('backs off with positive jitter while never shortening server Retry-After', () => {
  vi.spyOn(Math, 'random').mockReturnValue(0.5);
  expect(readRetryDelay(new Error('offline'), 1)).toBe(1125);
  expect(readRetryDelay(new Error('offline'), 2)).toBe(2250);
  expect(readRetryDelay(new Error('offline'), 20)).toBe(30500);
  expect(readRetryDelay(new ApiError(429, 'Busy', undefined, undefined, 120_000), 1)).toBe(120500);
});
it.each(['15', new Date(Date.now() + 15_000).toUTCString()])(
  'preserves valid Retry-After %s',
  (header) => {
    const delay = retryAfterMilliseconds(
      new Response(null, { headers: { 'Retry-After': header } }),
    );
    expect(delay).toBeGreaterThan(13_000);
    expect(delay).toBeLessThanOrEqual(15_000);
  },
);
it.each(['', '-1', 'garbage', '999999999999999', new Date(0).toUTCString()])(
  'rejects unsafe Retry-After %s',
  (header) => {
    expect(
      retryAfterMilliseconds(new Response(null, { headers: { 'Retry-After': header } })),
    ).toBeUndefined();
  },
);

it('supplies one enforced retry schedule while preserving unrelated established cadence', async () => {
  vi.useFakeTimers();
  try {
    const { observeRead } = await import('./readObserver');
    const { retrySchedule, rateLimitDelay } = await import('./readRetry');
    vi.spyOn(Math, 'random').mockReturnValue(0);
    const error = new ApiError(429, 'Busy', undefined, undefined, 12_000);
    expect(rateLimitDelay(error, 5000)).toBe(12_000);
    expect(rateLimitDelay(new ApiError(503, 'Offline'), 5000)).toBe(5000);
    const read = vi.fn(async () => retrySchedule(error, 1));
    const observer = observeRead(read);
    await vi.advanceTimersByTimeAsync(0);
    for (let i = 0; i < 11; i++) {
      observer.refresh();
      observer.hint();
      await vi.advanceTimersByTimeAsync(1000);
    }
    expect(read).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(1000);
    expect(read).toHaveBeenCalledTimes(2);
    observer.dispose();
  } finally {
    vi.useRealTimers();
  }
});
