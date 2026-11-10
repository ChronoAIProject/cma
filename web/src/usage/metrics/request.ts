import { ApiError } from '@chronoai/cma-sdk';

const retryDeadlines = new WeakMap<object, number>();

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
        Date.now() + Math.max(1000, error.retryAfterMs ?? 60000),
      );
      retryDeadlines.set(scope, deadline);
      throw new UsageBackoff(deadline);
    }
    throw error;
  }
}
