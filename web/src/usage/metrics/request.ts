import { ApiError } from '@chronoai/cma-sdk';

const retryDeadlines = new WeakMap<object, number>();

/**
 * Spread clients around a server supplied retry deadline without retrying
 * before that deadline. The bounded random factor keeps Retry-After
 * authoritative while preventing synchronized clients from retrying together.
 */
export function jitteredRetryDelay(milliseconds: number, random = Math.random()): number {
  const base = Number.isFinite(milliseconds) ? Math.max(1000, milliseconds) : 1000;
  const sample = Math.min(1, Math.max(0, random));
  return Math.round(base * (1 + sample * 0.1));
}

/** A local refusal preserves the original deadline instead of extending Retry-After. */
export class UsageBackoff extends Error {
  constructor(readonly retryAt: number) {
    super('Usage reporting is busy');
  }
}

/** Share only cooldown metadata, never report data, across one client's mounted views. */
export async function usageRequest<T>(
  scope: object,
  signal: AbortSignal,
  load: (signal: AbortSignal) => Promise<T>,
): Promise<T> {
  signal.throwIfAborted();
  const retryAt = retryDeadlines.get(scope) ?? 0;
  if (retryAt > Date.now()) throw new UsageBackoff(retryAt);
  try {
    return await load(signal);
  } catch (error) {
    if (!signal.aborted && error instanceof ApiError && error.status === 429) {
      const deadline = Math.max(
        retryDeadlines.get(scope) ?? 0,
        Date.now() + jitteredRetryDelay(error.retryAfterMs ?? 60000),
      );
      retryDeadlines.set(scope, deadline);
      throw new UsageBackoff(deadline);
    }
    throw error;
  }
}
