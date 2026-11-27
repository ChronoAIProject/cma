import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ApiError, ProtocolError, TransportError } from '@cma/client';
import {
  requestRecoveryDiagnostics,
  transientRequestFailure,
  withRequestRecovery,
} from './requestRecovery';

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());
const policy = { enabled: true, delay: () => 100 };

it.each([429, 502, 503, 504])(
  'retries transient %s with the provided server delay floor',
  async (status) => {
    const error = new ApiError(status, 'Temporary', undefined, undefined, 1500);
    const run = vi.fn().mockRejectedValueOnce(error).mockResolvedValue('receipt');
    const delay = vi.fn((error: unknown) => (error as ApiError).retryAfterMs!);
    const result = withRequestRecovery(undefined, { enabled: true, delay }, (context) =>
      context.retry(run),
    );
    await vi.advanceTimersByTimeAsync(1499);
    expect(run).toHaveBeenCalledOnce();
    await vi.advanceTimersByTimeAsync(1);
    await expect(result).resolves.toBe('receipt');
    expect(run.mock.calls).toEqual([[0], [1]]);
  },
);

it.each([401, 403, 409, 422, 500])(
  'does not classify %s as automatic mutation recovery',
  async (status) => {
    const error = new ApiError(status, 'Refused');
    const run = vi.fn().mockRejectedValue(error);
    await expect(
      withRequestRecovery(undefined, policy, (context) => context.retry(run)),
    ).rejects.toBe(error);
    expect(run).toHaveBeenCalledOnce();
  },
);

it('keeps malformed receipts and unknown exceptions fatal', async () => {
  for (const error of [new ProtocolError('Invalid receipt'), new Error('Identity failed')]) {
    expect(transientRequestFailure(error)).toBe(false);
    await expect(
      withRequestRecovery(undefined, policy, (context) =>
        context.retry(async () => {
          throw error;
        }),
      ),
    ).rejects.toBe(error);
  }
});

it('preserves disabled behavior without timers or shortened request deadlines', async () => {
  const error = new TransportError('Lost response');
  const run = vi.fn().mockRejectedValue(error);
  await expect(
    withRequestRecovery(undefined, { ...policy, enabled: false }, (context) => {
      expect(context.timeoutMs).toBeUndefined();
      return context.retry(run);
    }),
  ).rejects.toBe(error);
  expect(run).toHaveBeenCalledOnce();
  expect(vi.getTimerCount()).toBe(0);
});

it.each([1000, 4000])('honors a %sms server floor for a scoped retry', async (delay) => {
  const error = new ApiError(503, 'Busy', 'response_control_busy', undefined, delay);
  const run = vi.fn().mockRejectedValueOnce(error).mockResolvedValue('receipt');
  const result = withRequestRecovery(undefined, { ...policy, enabled: false }, (context) =>
    context.retry(run, (failure) => failure === error),
  );
  await vi.advanceTimersByTimeAsync(delay - 1);
  expect(run).toHaveBeenCalledOnce();
  await vi.advanceTimersByTimeAsync(1);
  await expect(result).resolves.toBe('receipt');
  expect(run.mock.calls).toEqual([[0], [1]]);
});

it('blocks scoped retry dispatch if fresh authority outlasts the action budget', async () => {
  const error = new ApiError(503, 'Busy', 'response_control_busy', undefined, 4000);
  const post = vi.fn();
  const result = expect(
    withRequestRecovery(undefined, { ...policy, enabled: false }, (context) =>
      context.retry(
        async (retry) => {
          if (retry === 0) throw error;
          await new Promise<void>((resolve) => setTimeout(resolve, 22_000));
          context.checkDeadline();
          post();
        },
        (failure) => failure === error,
      ),
    ),
  ).rejects.toThrow('awaiting confirmation');
  await vi.advanceTimersByTimeAsync(26_000);
  await result;
  expect(post).not.toHaveBeenCalled();
});

it('shares four retries across independent stages of the same action', async () => {
  const error = new TransportError('Unavailable');
  const first = vi.fn().mockRejectedValueOnce(error).mockResolvedValue(undefined);
  const second = vi.fn().mockRejectedValue(error);
  const result = expect(
    withRequestRecovery(undefined, policy, async (context) => {
      await context.retry(first);
      await context.retry(second);
    }),
  ).rejects.toBe(error);
  await vi.runAllTimersAsync();
  await result;
  expect(first).toHaveBeenCalledTimes(2);
  expect(second).toHaveBeenCalledTimes(4);
  expect(vi.getTimerCount()).toBe(0);
});

it('does not shorten Retry-After beyond the action recovery budget', async () => {
  const error = new ApiError(429, 'Rate limited', undefined, undefined, 60_000);
  const run = vi.fn().mockRejectedValue(error);
  await expect(
    withRequestRecovery(undefined, { enabled: true, delay: () => 60_000 }, (context) =>
      context.retry(run),
    ),
  ).rejects.toBe(error);
  expect(run).toHaveBeenCalledOnce();
});

it('aborts a pending retry sleep without issuing another attempt', async () => {
  const caller = new AbortController();
  const run = vi.fn().mockRejectedValue(new TransportError('Disconnected'));
  const result = expect(
    withRequestRecovery(caller.signal, policy, (context) => context.retry(run)),
  ).rejects.toMatchObject({ name: 'AbortError' });
  await vi.advanceTimersByTimeAsync(50);
  caller.abort();
  await result;
  await vi.runAllTimersAsync();
  expect(run).toHaveBeenCalledOnce();
  expect(vi.getTimerCount()).toBe(0);
});

it('includes fresh authority work in the total action deadline', async () => {
  const before = requestRecoveryDiagnostics();
  const result = expect(
    withRequestRecovery(undefined, policy, async (context) => {
      expect(context.timeoutMs).toBe(5000);
      await new Promise<void>((_resolve, reject) =>
        context.signal.addEventListener('abort', () => reject(context.signal.reason), {
          once: true,
        }),
      );
    }),
  ).rejects.toThrow('awaiting confirmation');
  await vi.advanceTimersByTimeAsync(25_000);
  await result;
  expect(requestRecoveryDiagnostics().exhausted).toBe(before.exhausted + 1);
});

it('rejects already aborted callers and invalid delay policies', async () => {
  const caller = new AbortController();
  caller.abort();
  const operation = vi.fn();
  await expect(withRequestRecovery(caller.signal, policy, operation)).rejects.toMatchObject({
    name: 'AbortError',
  });
  expect(operation).not.toHaveBeenCalled();
  await expect(
    withRequestRecovery(undefined, { enabled: true, delay: () => NaN }, (context) =>
      context.retry(async () => {
        throw new TransportError('Lost');
      }),
    ),
  ).rejects.toThrow('finite and nonnegative');
});
