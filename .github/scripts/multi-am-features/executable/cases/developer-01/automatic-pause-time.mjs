import {setTimeout as delay} from 'node:timers/promises';
import {check, required} from './common.mjs';

export const realClock = {now: () => performance.now(), wall: () => Date.now(),
  sleep: (ms, signal) => delay(ms, undefined, {signal})};
const GRACE_MS = 60000, RESERVE_MS = 30000;

export function automaticBudget(ctx) {
  const remaining = typeof ctx.remainingMs === 'function' ? ctx.remainingMs() : null;
  required(Number.isSafeInteger(remaining) && remaining > GRACE_MS + RESERVE_MS,
    'admin_auto_pause_budget');
}

export async function waitAutomaticGrace(ctx, stored, clock = realClock) {
  const policy = stored.settings?.inactivity;
  check(policy?.auto_pause_enabled === true && policy.pause_after_seconds === 60
    && typeof policy.grace_started_at === 'string' && Number.isFinite(Date.parse(policy.grace_started_at)),
  'admin_auto_pause_server_grace');
  automaticBudget(ctx);
  // Start after the real enable response and peer readbacks. A full monotonic
  // interval is conservative even when the server and harness clocks differ.
  const start = clock.now(), startedAt = new Date(clock.wall()).toISOString();
  for (;;) {
    ctx.signal.throwIfAborted();
    const elapsed = clock.now() - start;
    check(Number.isFinite(elapsed) && elapsed >= 0, 'admin_auto_pause_clock');
    if (elapsed >= GRACE_MS) return ctx.observe('admin_auto_pause_grace', async () => ({
      revision: stored.revision, grace_started_at: policy.grace_started_at,
      threshold_seconds: 60, elapsed_ms: elapsed, started_at: startedAt,
      ended_at: new Date(clock.wall()).toISOString(), basis: 'monotonic_after_enable_readback',
    }));
    check(ctx.remainingMs() > GRACE_MS - elapsed + RESERVE_MS, 'admin_auto_pause_wait_budget');
    await clock.sleep(Math.min(1000, GRACE_MS - elapsed), ctx.signal);
  }
}

export async function automaticPoll(ctx, operation, complete, clock = realClock) {
  const deadline = clock.now() + Math.min(120000, Math.max(0, ctx.remainingMs() - 15000));
  for (;;) {
    ctx.signal.throwIfAborted();
    const value = await operation();
    if (complete(value)) return value;
    check(clock.now() < deadline, 'admin_auto_pause_timeout');
    await clock.sleep(Math.min(1000, deadline - clock.now()), ctx.signal);
  }
}
