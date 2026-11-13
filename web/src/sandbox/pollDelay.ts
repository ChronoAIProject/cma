import { readinessDelay } from '../agent/readinessWait';
import type { SandboxDetail } from '../api/types';

export function sandboxPollKey(sandbox: SandboxDetail) {
  return JSON.stringify([sandbox.status, sandbox.lifecycle, sandbox.runtimeUpdate]);
}

export const PROLONGED_POLL_MS = 15_000;
export const LONG_POLL_MS = 30_000;

export function unchangedPollMaximum(unchangedFor: number) {
  if (unchangedFor >= 600_000) return LONG_POLL_MS;
  if (unchangedFor >= 120_000) return PROLONGED_POLL_MS;
  return 5500;
}

/** One observer's cadence; a new lifecycle observation starts readiness again. */
export function createPollDelay(maximum = 5500, prolonged = false) {
  let previous: string | undefined;
  let attempt = 0;
  let unchangedSince = 0;
  return {
    reset() {
      previous = undefined;
    },
    next(key: string, base: number) {
      attempt = key === previous ? attempt + 1 : 0;
      if (key !== previous) unchangedSince = Date.now();
      previous = key;
      // Settled observations keep their separate five-second lifecycle check.
      if (base !== 1500) return base;
      const limit = prolonged ? unchangedPollMaximum(Date.now() - unchangedSince) : 5500;
      // Long-lived transitions have a hard ceiling; the short readiness steps
      // retain the composer's jitter and its existing 5.5-second maximum.
      if (prolonged && Date.now() - unchangedSince >= 120_000) return Math.min(limit, maximum);
      return Math.min(attempt > 0 ? readinessDelay(attempt, 1500) : base, maximum);
    },
  };
}

export function createPagePollDelay() {
  return createPollDelay(LONG_POLL_MS, true);
}
