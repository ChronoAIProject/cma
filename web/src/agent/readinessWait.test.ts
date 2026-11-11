import { afterEach, expect, it, vi } from 'vitest';
import { ReadinessWake, readinessDelay } from './readinessWait';

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

it('backs off with bounded jitter and never waits more than 5.5 seconds', () => {
  vi.spyOn(Math, 'random').mockReturnValue(0);
  expect([0, 1, 2, 3, 50].map((n) => readinessDelay(n))).toEqual([900, 1800, 3600, 4500, 4500]);
  vi.mocked(Math.random).mockReturnValue(1);
  expect(readinessDelay(50)).toBe(5500);
});

it('coalesces event bursts without postponing a pending wake or polling faster than one second', async () => {
  vi.useFakeTimers();
  const wake = new ReadinessWake();
  const done = vi.fn();
  const wait = wake.wait(5000, new AbortController().signal).then(done);
  for (let i = 0; i < 10; i++) {
    wake.hint();
    await vi.advanceTimersByTimeAsync(100);
  }
  await wait;
  expect(done).toHaveBeenCalledTimes(1);
});

it('suspends hidden readiness, wakes on visibility, and cancels without leaked listeners', async () => {
  vi.useFakeTimers();
  const hidden = vi.spyOn(document, 'hidden', 'get').mockReturnValue(true);
  const wake = new ReadinessWake();
  const done = vi.fn();
  const abort = new AbortController();
  const wait = wake.wait(1000, abort.signal).then(done);
  await vi.advanceTimersByTimeAsync(30000);
  wake.hint();
  expect(done).not.toHaveBeenCalled();
  hidden.mockReturnValue(false);
  document.dispatchEvent(new Event('visibilitychange'));
  await vi.advanceTimersByTimeAsync(0);
  await wait;
  expect(done).toHaveBeenCalledOnce();
  const pending = wake.wait(5000, abort.signal).catch((error: unknown) => error);
  abort.abort();
  expect(await pending).toBe(abort.signal.reason);
  wake.hint();
  expect(vi.getTimerCount()).toBe(0);
});
