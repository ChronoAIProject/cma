import { afterEach, expect, it, vi } from 'vitest';
import { sandboxFixture } from '../test/hierarchy';
import { createPagePollDelay, createPollDelay, sandboxPollKey } from './pollDelay';

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

it.each([0, 0.5, 1])('preserves composer jitter and the 5.5-second ceiling (%s)', (random) => {
  vi.useFakeTimers();
  vi.spyOn(Math, 'random').mockReturnValue(random);
  const delay = createPollDelay();
  expect(delay.next('starting', 1500)).toBe(1500);
  expect(delay.next('starting', 1500)).toBeCloseTo(3000 * (0.9 + random * 0.2));
  for (let i = 0; i < 100; i++) {
    vi.advanceTimersByTime(60_000);
    expect(delay.next('starting', 1500)).toBeCloseTo(5000 * (0.9 + random * 0.2));
  }
  expect(delay.next('ready', 5000)).toBe(5000);
  expect(delay.next('ready', 5000)).toBe(5000);
});

it('resets page backoff for lifecycle or runtime progress and explicit refresh', () => {
  vi.useFakeTimers();
  const delay = createPagePollDelay();
  const sandbox = sandboxFixture('credential_rotating');
  const key = sandboxPollKey(sandbox);
  expect(delay.next(key, 1500)).toBe(1500);
  vi.advanceTimersByTime(120_000);
  expect(delay.next(key, 1500)).toBe(15_000);
  vi.advanceTimersByTime(480_000);
  expect(delay.next(key, 1500)).toBe(30_000);
  const changed = sandboxPollKey({
    ...sandbox,
    lifecycle: { ...sandbox.lifecycle!, pause: { allowed: false, reason: 'work_in_progress' } },
  });
  expect(delay.next(changed, 1500)).toBe(1500);
  vi.advanceTimersByTime(600_000);
  expect(delay.next(changed, 1500)).toBe(30_000);
  delay.reset();
  expect(delay.next(changed, 1500)).toBe(1500);
  // Runtime update fields are part of the same observation key, even when
  // the lifecycle status itself has not changed.
  expect(
    sandboxPollKey({
      ...sandbox,
      runtimeUpdate: {
        currentVersion: 'old',
        latestVersion: 'new',
        updateAvailable: true,
        state: 'updating',
        canUpdate: false,
        preservesHome: true,
        schedule: { timezone: 'Asia/Singapore', localTime: '02:00' },
      },
    }),
  ).not.toBe(key);
});

it('honors a caller ceiling without changing the settled cadence', () => {
  const delay = createPollDelay(1500);
  for (let i = 0; i < 10; i++) expect(delay.next('unchanged', 1500)).toBe(1500);
  expect(delay.next('ready', 5000)).toBe(5000);
});
