import { afterEach, expect, it, vi } from 'vitest';
import { ApiError } from '@chronoai/cma-sdk';
import { metricsFailure } from './failure';
import { UsageBackoff, usageRequest } from './request';

afterEach(() => vi.useRealTimers());

it('shares Retry-After across reads without extending it or sharing account data', async () => {
  vi.useFakeTimers();
  const scope = {};
  const signal = new AbortController().signal;
  const busy = vi.fn().mockRejectedValue(new ApiError(429, 'Busy', undefined, undefined, 30000));
  const deadline = Date.now() + 30000;
  await expect(usageRequest(scope, signal, busy)).rejects.toMatchObject({ retryAt: deadline });
  const next = vi.fn().mockResolvedValue('ready');
  await vi.advanceTimersByTimeAsync(29000);
  await expect(usageRequest(scope, signal, next)).rejects.toMatchObject({ retryAt: deadline });
  expect(next).not.toHaveBeenCalled();
  expect(metricsFailure(new UsageBackoff(deadline)).retryAt).toBe(deadline);
  await expect(usageRequest({}, signal, next)).resolves.toBe('ready');
  await vi.advanceTimersByTimeAsync(1000);
  await expect(usageRequest(scope, signal, next)).resolves.toBe('ready');
  expect(next).toHaveBeenCalledTimes(2);
});

it.each([401, 403, 404, 410, 503, 500])(
  'preserves the real %s failure after backoff',
  async (status) => {
    vi.useFakeTimers();
    const scope = {};
    const signal = new AbortController().signal;
    await expect(
      usageRequest(scope, signal, async () => {
        throw new ApiError(429, 'Busy');
      }),
    ).rejects.toBeInstanceOf(UsageBackoff);
    await vi.advanceTimersByTimeAsync(60000);
    const error = new ApiError(status, 'Refused');
    await expect(
      usageRequest(scope, signal, async () => {
        throw error;
      }),
    ).rejects.toBe(error);
  },
);

it('does not send an aborted read or let its late response impose a cooldown', async () => {
  const scope = {};
  const controller = new AbortController();
  let reject!: (error: Error) => void;
  const pending = usageRequest(
    scope,
    controller.signal,
    () =>
      new Promise((_resolve, fail) => {
        reject = fail;
      }),
  );
  controller.abort();
  reject(new ApiError(429, 'Busy'));
  await expect(pending).rejects.toBeInstanceOf(ApiError);
  const load = vi.fn().mockResolvedValue('ready');
  await expect(usageRequest(scope, controller.signal, load)).rejects.toMatchObject({
    name: 'AbortError',
  });
  expect(load).not.toHaveBeenCalled();
  await expect(usageRequest(scope, new AbortController().signal, load)).resolves.toBe('ready');
});
