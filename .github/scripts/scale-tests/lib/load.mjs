import { check, clock, finite } from './checks.mjs';

export const connectedWorkload = Object.freeze({ users: 100, streams: 100, readsPerSecond: 10, turnsPerMinute: 20 });

export function startLoad({ read, turn, durationMs, maxInFlight = 100, time = clock,
  readsPerSecond = 10, turnsPerMinute = 20 }) {
  for (const value of [durationMs, readsPerSecond, turnsPerMinute]) finite(value);
  check(durationMs > 0 && readsPerSecond > 0 && turnsPerMinute > 0
    && Number.isInteger(maxInFlight) && maxInFlight > 0, 'invalid_load_budget');
  const pending = new Set();
  const samples = [];
  const started = time.now();
  let stopped = false;
  const dispatch = (kind, operation, scheduled) => {
    if (pending.size >= maxInFlight) {
      samples.push({ kind, ok: false, elapsedMs: time.now() - scheduled, code: 'generator_overloaded' });
      return;
    }
    const task = (async () => {
      try {
        await operation({ scheduledAt: scheduled, signal: controller.signal });
        samples.push({ kind, ok: true, elapsedMs: time.now() - scheduled });
      } catch {
        samples.push({ kind, ok: false, elapsedMs: time.now() - scheduled, code: 'request_failed' });
      }
    })();
    pending.add(task);
    task.then(() => pending.delete(task));
  };
  const controller = new AbortController();
  const done = (async () => {
    let nextRead = started; let nextTurn = started;
    while (!stopped && time.now() - started < durationMs) {
      const now = time.now();
      // Scheduled-start latency includes generator queueing (no coordinated omission).
      while (nextRead <= now && nextRead < started + durationMs) {
        dispatch('read', read, nextRead); nextRead += 1000 / readsPerSecond;
      }
      while (nextTurn <= now && nextTurn < started + durationMs) {
        dispatch('turn', turn, nextTurn); nextTurn += 60000 / turnsPerMinute;
      }
      await time.sleep(Math.max(1, Math.min(nextRead, nextTurn, started + durationMs) - time.now()));
    }
    await Promise.all(pending);
    return samples;
  })();
  return { done, stop: async () => { stopped = true; return done; },
    abort: () => { stopped = true; controller.abort(); } };
}
