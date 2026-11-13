import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { observeRead, type ReadSchedule } from './readObserver';

beforeEach(() => vi.useFakeTimers());
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});
const advance = (ms: number) => vi.advanceTimersByTimeAsync(ms);

it('bounds a continuous event stream without starving reads or overlapping them', async () => {
  const read = vi.fn(async () => ({ next: 2000, cooldown: 2000 }));
  const observer = observeRead(read);
  await advance(0);
  for (let n = 0; n < 300; n++) {
    observer.hint();
    await advance(200);
  }
  expect(read).toHaveBeenCalledTimes(31);
  observer.dispose();
});

it('coalesces in-flight hints and refreshes without hiding an explicit mutation refresh', async () => {
  let finish!: (value: ReadSchedule) => void;
  const read = vi
    .fn<() => Promise<ReadSchedule>>()
    .mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    )
    .mockResolvedValue({ cooldown: 2000 });
  const observer = observeRead(read);
  for (let n = 0; n < 20; n++) {
    observer.hint();
    observer.refresh();
    await advance(100);
  }
  expect(read).toHaveBeenCalledTimes(1);
  finish({ cooldown: 2000 });
  await advance(100);
  expect(read).toHaveBeenCalledTimes(2);
  await advance(2000);
  expect(read).toHaveBeenCalledTimes(2);
  observer.dispose();
});

it('preserves failure backoff despite stream, focus and explicit refresh storms', async () => {
  const read = vi.fn(async () => ({ next: 10000, cooldown: 10000, failed: true }));
  const observer = observeRead(read);
  await advance(0);
  for (let n = 0; n < 99; n++) {
    observer.hint();
    observer.refresh();
    window.dispatchEvent(new Event('focus'));
    await advance(100);
  }
  expect(read).toHaveBeenCalledTimes(1);
  await advance(100);
  expect(read).toHaveBeenCalledTimes(2);
  observer.dispose();
});

it('suspends hidden polling and coalesces visible/focus catch-up', async () => {
  let visible = false;
  vi.spyOn(document, 'visibilityState', 'get').mockImplementation(() =>
    visible ? 'visible' : 'hidden',
  );
  const read = vi.fn(async () => ({ next: 2000, cooldown: 2000 }));
  const observer = observeRead(read);
  await advance(60000);
  expect(read).not.toHaveBeenCalled();
  visible = true;
  document.dispatchEvent(new Event('visibilitychange'));
  window.dispatchEvent(new Event('focus'));
  await advance(0);
  expect(read).toHaveBeenCalledTimes(1);
  visible = false;
  document.dispatchEvent(new Event('visibilitychange'));
  await advance(60000);
  expect(read).toHaveBeenCalledTimes(1);
  visible = true;
  document.dispatchEvent(new Event('visibilitychange'));
  window.dispatchEvent(new Event('focus'));
  await advance(0);
  expect(read).toHaveBeenCalledTimes(2);
  observer.dispose();
});

it('stops terminal reads and aborts late work on disposal', async () => {
  const read = vi.fn(async () => undefined);
  const observer = observeRead(read);
  await advance(0);
  observer.hint();
  observer.refresh();
  window.dispatchEvent(new Event('focus'));
  await advance(60000);
  expect(read).toHaveBeenCalledTimes(1);
  observer.dispose();
  let finish!: (value: ReadSchedule) => void;
  const pending = observeRead(
    (signal) =>
      new Promise((resolve) => {
        signal.addEventListener('abort', () => expect(signal.aborted).toBe(true));
        finish = resolve;
      }),
  );
  pending.dispose();
  finish({ next: 100, cooldown: 100 });
  await advance(1000);
  expect(vi.getTimerCount()).toBe(0);
});
