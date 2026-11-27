import { ApiError, TransportError } from '@cma/client';

export interface RecoveryPolicy {
  readonly enabled: boolean;
  readonly delay: (error: unknown, failures: number) => number;
}

export interface RequestRecovery {
  readonly signal: AbortSignal;
  readonly timeoutMs?: number;
  readonly retried: boolean;
  checkDeadline(): void;
  /** An operation can authorize one retry for a specific server refusal. */
  retry<T>(
    operation: (retry: number) => Promise<T>,
    retryOnce?: (error: unknown) => boolean,
  ): Promise<T>;
}

const budgetMs = 25_000;
const maxRetries = 4;
const counts = { retries: 0, exhausted: 0, recovered: 0 };

/** Fixed counters only; never retained request keys, resource IDs or input text. */
export const requestRecoveryDiagnostics = () => ({ ...counts });

export function transientRequestFailure(error: unknown): boolean {
  return (
    error instanceof TransportError ||
    (error instanceof ApiError && [429, 502, 503, 504].includes(error.status))
  );
}

function expired(): Error {
  return new Error(
    'This request is still awaiting confirmation. Recover it before submitting again.',
  );
}

/** One deadline and retry budget cover the whole action, including its fresh checks. */
export async function withRequestRecovery<T>(
  signal: AbortSignal | undefined,
  policy: RecoveryPolicy,
  operation: (recovery: RequestRecovery) => Promise<T>,
): Promise<T> {
  const controller = new AbortController();
  const abort = () => controller.abort(signal?.reason);
  signal?.throwIfAborted();
  signal?.addEventListener('abort', abort, { once: true });
  const deadline = performance.now() + budgetMs;
  let timer = policy.enabled ? setTimeout(() => controller.abort(expired()), budgetMs) : undefined;
  let retries = 0;
  let exhausted = false;
  const exhaust = () => {
    if (!exhausted) counts.exhausted += 1;
    exhausted = true;
  };
  const checkDeadline = () => {
    if (timer !== undefined && performance.now() >= deadline) controller.abort(expired());
    controller.signal.throwIfAborted();
  };
  const context: RequestRecovery = {
    signal: controller.signal,
    get retried() {
      return retries > 0;
    },
    get timeoutMs() {
      return timer !== undefined
        ? Math.max(1, Math.min(5000, Math.floor(deadline - performance.now())))
        : undefined;
    },
    checkDeadline,
    async retry<T>(
      attempt: (retry: number) => Promise<T>,
      retryOnce?: (error: unknown) => boolean,
    ): Promise<T> {
      let localRetries = 0;
      for (;;) {
        controller.signal.throwIfAborted();
        try {
          const result = await attempt(localRetries);
          controller.signal.throwIfAborted();
          if (localRetries > 0) counts.recovered += 1;
          return result;
        } catch (error) {
          const generalRecovery = policy.enabled && transientRequestFailure(error);
          const specificRetry = !generalRecovery && retries === 0 && retryOnce?.(error) === true;
          if (controller.signal.aborted || (!generalRecovery && !specificRetry)) throw error;
          const serverFloor = error instanceof ApiError ? (error.retryAfterMs ?? 0) : 0;
          const delay = generalRecovery
            ? policy.delay(error, retries + 1)
            : Math.max(1000, serverFloor);
          if (!Number.isFinite(delay) || delay < 0)
            throw new Error('Request recovery delay must be finite and nonnegative.', {
              cause: error,
            });
          // A server Retry-After longer than this action's budget is never shortened.
          const remaining = deadline - performance.now();
          if (retries >= maxRetries || delay >= remaining) {
            exhaust();
            throw error;
          }
          timer ??= setTimeout(() => controller.abort(expired()), remaining);
          await wait(delay, controller.signal);
          checkDeadline();
          retries += 1;
          localRetries += 1;
          counts.retries += 1;
        }
      }
    },
  };
  try {
    const result = await operation(context);
    controller.signal.throwIfAborted();
    return result;
  } catch (error) {
    if (controller.signal.aborted) {
      if (!signal?.aborted) exhaust();
      throw controller.signal.reason;
    }
    throw error;
  } finally {
    clearTimeout(timer);
    signal?.removeEventListener('abort', abort);
    controller.abort();
  }
}

async function wait(ms: number, signal: AbortSignal): Promise<void> {
  signal.throwIfAborted();
  await new Promise<void>((resolve, reject) => {
    const abort = () => {
      clearTimeout(timer);
      signal.removeEventListener('abort', abort);
      reject(signal.reason);
    };
    const timer = setTimeout(() => {
      signal.removeEventListener('abort', abort);
      resolve();
    }, ms);
    signal.addEventListener('abort', abort, { once: true });
  });
}
