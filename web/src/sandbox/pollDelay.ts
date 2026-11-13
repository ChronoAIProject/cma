import { readinessDelay } from '../agent/readinessWait';
import type { SandboxDetail } from '../api/types';

export function sandboxPollKey(sandbox: SandboxDetail) {
  return JSON.stringify([sandbox.status, sandbox.lifecycle, sandbox.runtimeUpdate]);
}

/** One observer's cadence; a new lifecycle observation starts readiness again. */
export function createPollDelay(maximum = 5500) {
  let previous: string | undefined;
  let attempt = 0;
  return {
    next(key: string, base: number) {
      attempt = key === previous ? attempt + 1 : 0;
      previous = key;
      // Settled observations keep their separate five-second lifecycle check.
      return base === 1500 && attempt > 0
        ? Math.min(readinessDelay(attempt, 1500), maximum)
        : base;
    },
  };
}
