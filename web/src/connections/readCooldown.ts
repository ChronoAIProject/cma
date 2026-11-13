import { ApiError } from '@chronoai/cma-sdk';
import { readRetryDelay } from '../agent/transport/readRetry';

interface ReadCooldown {
  readonly owner?: string;
  failures: number;
  until: number;
  error?: ApiError;
}

const cooldowns = new WeakMap<object, ReadCooldown>();
const counts = { attempted: 0, deferred: 0 };
export const connectionReadDiagnostics = () => ({ ...counts });

/** A failed read's deadline survives remount/retry, without caching readiness or authority. */
export async function readWithConnectionCooldown<Value>(
  context: object | undefined,
  owner: string | undefined,
  signal: AbortSignal,
  read: () => Promise<Value>,
): Promise<Value> {
  signal.throwIfAborted();
  let state = context === undefined ? undefined : cooldowns.get(context);
  if (!state || state.owner !== owner) {
    state = { owner, failures: 0, until: 0 };
    if (context !== undefined) cooldowns.set(context, state);
  }
  if (state.error && performance.now() < state.until) {
    counts.deferred += 1;
    throw state.error;
  }
  counts.attempted += 1;
  try {
    const result = await read();
    signal.throwIfAborted();
    // Success cannot erase a newer overlapping failure's server deadline.
    if (performance.now() >= state.until) {
      state.failures = 0;
      state.error = undefined;
    }
    return result;
  } catch (error) {
    if (!signal.aborted && error instanceof ApiError && error.isRetryable) {
      state.failures += 1;
      const until = performance.now() + readRetryDelay(error, state.failures);
      if (until >= state.until) {
        state.until = until;
        state.error = error;
      }
    }
    throw error;
  }
}
