import { ApiError, TimeoutError, TransportError } from '@cma/client';
import { jitteredRetryDelay, UsageBackoff, usageRequest } from './request';

export interface UsageRetryState {
  failures: number;
  retryAt: number;
}
export const REPORT_OBSERVATION_MS = 120_000;
const REQUEST_TIMEOUT_MS = 30_000;

export class UsageObservationLimit extends Error {
  constructor() {
    super('The report is taking longer than expected');
  }
}

export function waitForUsageRetry(milliseconds: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    const aborted = () => {
      clearTimeout(timer);
      reject(signal.reason);
    };
    const timer = setTimeout(
      () => {
        signal.removeEventListener('abort', aborted);
        resolve();
      },
      Math.min(2_147_483_647, Math.max(0, milliseconds)),
    );
    signal.addEventListener('abort', aborted, { once: true });
    if (signal.aborted) aborted();
  });
}

export function isTransientUsageFailure(error: unknown): boolean {
  return (
    error instanceof UsageBackoff ||
    error instanceof TransportError ||
    (error instanceof ApiError && [408, 425, 429, 500, 502, 503, 504].includes(error.status))
  );
}

/** Abort ownership and a local timeout also fence clients which settle after cancellation. */
async function boundedRead<T>(
  signal: AbortSignal,
  load: (signal: AbortSignal) => Promise<T>,
): Promise<T> {
  signal.throwIfAborted();
  const controller = new AbortController();
  const cancel = () => controller.abort(signal.reason);
  signal.addEventListener('abort', cancel, { once: true });
  const timer = setTimeout(
    () => controller.abort(new TimeoutError(REQUEST_TIMEOUT_MS)),
    REQUEST_TIMEOUT_MS,
  );
  let aborted!: () => void;
  try {
    return await Promise.race([
      Promise.resolve().then(() => {
        controller.signal.throwIfAborted();
        return load(controller.signal);
      }),
      new Promise<never>((_, reject) => {
        aborted = () => reject(controller.signal.reason);
        controller.signal.addEventListener('abort', aborted, { once: true });
        if (controller.signal.aborted) aborted();
      }),
    ]);
  } finally {
    clearTimeout(timer);
    signal.removeEventListener('abort', cancel);
    controller.signal.removeEventListener('abort', aborted);
  }
}

/** Retry only classified transient reads; successful authorization is never cached here. */
export async function retryUsageRead<T>(
  scope: object,
  signal: AbortSignal,
  load: (signal: AbortSignal) => Promise<T>,
  state: UsageRetryState = { failures: 0, retryAt: 0 },
  changed: (waiting: boolean) => void = () => {},
  maxRetries = 3,
): Promise<T> {
  while (true) {
    if (state.retryAt > Date.now()) {
      changed(true);
      await waitForUsageRetry(state.retryAt - Date.now(), signal);
    }
    signal.throwIfAborted();
    try {
      const value = await usageRequest(scope, signal, (requestSignal) =>
        boundedRead(requestSignal, load),
      );
      signal.throwIfAborted();
      state.failures = 0;
      state.retryAt = 0;
      changed(false);
      return value;
    } catch (error) {
      signal.throwIfAborted();
      if (!isTransientUsageFailure(error)) throw error;
      state.failures += 1;
      const floor =
        error instanceof UsageBackoff
          ? error.retryAt
          : Date.now() + (error instanceof ApiError ? (error.retryAfterMs ?? 0) : 0);
      state.retryAt = Math.max(
        floor,
        Date.now() + jitteredRetryDelay(Math.min(30_000, 1000 * 2 ** (state.failures - 1))),
      );
      if (state.failures > maxRetries) throw error;
      // Status and attempt counts are useful diagnostics without report IDs or subjects.
      console.debug('Retrying usage read', {
        attempt: state.failures,
        status: error instanceof ApiError ? error.status : undefined,
        delayMs: state.retryAt - Date.now(),
      });
      changed(true);
    }
  }
}
