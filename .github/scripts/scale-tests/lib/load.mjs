import { check, clock, finite } from './checks.mjs';

export const connectedWorkload = Object.freeze({ users: 100, streams: 100, readsPerSecond: 10, turnsPerMinute: 20 });

export function startLoad({ read, turn, durationMs, maxInFlight = 100, time = clock,
  readsPerSecond = 10, turnsPerMinute = 20, signal, onSample = () => {} }) {
  for (const value of [durationMs, readsPerSecond, turnsPerMinute]) finite(value);
  check(durationMs > 0 && readsPerSecond > 0 && turnsPerMinute > 0
    && Number.isInteger(maxInFlight) && maxInFlight > 0, 'invalid_load_budget');
  const pending = new Set();
  const samples = [];
  const started = time.now();
  const deadline = started + durationMs;
  let stopped = false;
  const record = (sample) => {
    samples.push(sample);
    onSample(sample);
  };
  const dispatch = (kind, operation, scheduled) => {
    if (pending.size >= maxInFlight) {
      record({ kind, ok: false, elapsedMs: time.now() - scheduled, code: 'generator_overloaded' });
      return;
    }
    const task = (async () => {
      try {
        await operation({ scheduledAt: scheduled, signal: workSignal });
        record({ kind, ok: true, elapsedMs: time.now() - scheduled });
      } catch {
        record({ kind, ok: false, elapsedMs: time.now() - scheduled, code: 'request_failed' });
      }
    })();
    pending.add(task);
    task.then(() => pending.delete(task));
  };
  const controller = new AbortController();
  const workSignal = signal ? AbortSignal.any([signal, controller.signal]) : controller.signal;
  const isScheduling = () => !stopped && !workSignal.aborted && time.now() < deadline;
  const done = (async () => {
    let nextRead = started; let nextTurn = started;
    while (isScheduling()) {
      const now = time.now();
      // Scheduled-start latency includes generator queueing (no coordinated omission).
      while (nextRead <= now && nextRead < deadline) {
        dispatch('read', read, nextRead); nextRead += 1000 / readsPerSecond;
      }
      while (nextTurn <= now && nextTurn < deadline) {
        dispatch('turn', turn, nextTurn); nextTurn += 60000 / turnsPerMinute;
      }
      try {
        await time.sleep(Math.max(1, Math.min(nextRead, nextTurn, deadline) - time.now()),
          undefined, { signal: workSignal });
      } catch (error) {
        if (!workSignal.aborted) throw error;
      }
    }
    await Promise.all(pending);
    return samples;
  })();
  return { done, isScheduling, stop: async () => { stopped = true; return done; },
    abort: () => { stopped = true; controller.abort(); } };
}
