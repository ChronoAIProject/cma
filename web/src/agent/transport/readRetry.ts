import { ApiError } from '@chronoai/cma-sdk';

/** A server deadline is a floor, including for focus/manual retry hints. */
export function readRetryDelay(error: unknown, failures: number, base = 1000): number {
  const backoff = Math.min(30_000, base * 2 ** Math.min(Math.max(0, failures - 1), 5));
  const server = error instanceof ApiError ? error.retryAfterMs : undefined;
  const floor = Math.max(backoff, server ?? 0);
  return Math.min(2_147_483_647, Math.ceil(floor + Math.random() * Math.min(floor / 4, 1000)));
}

export function retrySchedule(error: unknown, failures: number, base = 1000) {
  const delay = readRetryDelay(error, failures, base);
  return { next: delay, cooldown: delay, failed: true as const };
}

/** Preserve an existing non-rate-limit cadence while respecting a shared 429. */
export function rateLimitDelay(error: unknown, fallback: number, failures = 1): number {
  return error instanceof ApiError && error.status === 429
    ? readRetryDelay(error, failures, fallback)
    : fallback;
}
