import { createRequire } from 'node:module';
import type { TestInfo } from '@playwright/test';

const require = createRequire(import.meta.url);

// Playwright 1.58.2 shares its after-hooks slot with earlier hooks. The public
// timeout is not the remaining allocation; keep this pinned boundary fail-closed.
export function teardownRemaining(info: TestInfo): number {
  try {
    const manager: unknown = Reflect.get(info, '_timeoutManager');
    if (typeof manager !== 'object' || manager === null) return 0;
    const deadline: unknown = Reflect.get(manager, 'currentSlotDeadline');
    if (typeof deadline !== 'function') return 0;
    const until: unknown = Reflect.apply(deadline, manager, []);
    const utils: unknown = require('playwright-core/lib/utils');
    if (typeof utils !== 'object' || utils === null) return 0;
    const clock: unknown = Reflect.get(utils, 'monotonicTime');
    if (typeof clock !== 'function') return 0;
    const now: unknown = Reflect.apply(clock, utils, []);
    return typeof until === 'number' && Number.isFinite(until) && typeof now === 'number'
      ? Math.max(0, until - now)
      : 0;
  } catch {
    return 0;
  }
}

export function teardownWindow(remainingMs: number) {
  const available = Number.isFinite(remainingMs) && remainingMs > 0 ? remainingMs : 0;
  const margin = Math.min(250, available / 4);
  const releaseMs = Math.min(2_000, (available - margin) / 3);
  return {
    observationMs: Math.min(5_000, available - margin - releaseMs),
    releaseMs,
  };
}

export async function settlesWithin(work: Promise<unknown>, milliseconds: number) {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      work.then(() => true),
      new Promise<false>((resolve) => {
        timer = setTimeout(() => resolve(false), Math.max(0, milliseconds));
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}
