import { ApiError } from '@cma/client';
import { afterEach, expect, it, vi } from 'vitest';
import {
  checkDesktopBudget,
  failDesktopAttempt,
  newDesktopReconnect,
  startDesktopAttempt,
} from './desktopReconnect';

afterEach(() => vi.restoreAllMocks());

it.each([0, 0.5, 0.999])('bounds exponential backoff and jitter at random input %s', (random) => {
  vi.spyOn(Math, 'random').mockReturnValue(random);
  let state = startDesktopAttempt(newDesktopReconnect(), 0);
  let now = 0;
  for (const floor of [1000, 2000, 4000, 8000, 16000, 30000, 30000]) {
    state = failDesktopAttempt(state, now);
    const delay = state.retryAt - now;
    expect(delay).toBe(Math.ceil(floor + random * Math.min(floor / 4, 1000)));
    expect(delay).toBeGreaterThanOrEqual(floor);
    expect(delay).toBeLessThanOrEqual(31_000);
    expect(state.expiresAt).toBe(300_000);
    now = state.retryAt;
    state = startDesktopAttempt(state, now);
    expect(state.terminal).toBeUndefined();
  }
});

it('checks the deadline before admitting another attempt without renewing it', () => {
  const state = failDesktopAttempt(startDesktopAttempt(newDesktopReconnect(), 0), 1000);
  expect(checkDesktopBudget(state, 300_999)).toBe(state);
  const stopped = startDesktopAttempt(state, 301_000);
  expect(stopped.terminal).toBe('exhausted');
  expect(stopped.automaticAttempts).toBe(0);
  expect(startDesktopAttempt(stopped, 302_000)).toBe(stopped);
  expect(failDesktopAttempt(stopped, 302_000)).toBe(stopped);
});

it('preserves a server cooldown when manually resetting an expired budget', () => {
  vi.spyOn(Math, 'random').mockReturnValue(0.5);
  const state = failDesktopAttempt(
    startDesktopAttempt(newDesktopReconnect(), 0),
    0,
    new ApiError(503, 'Busy', 'desktop_runtime_unavailable', undefined, 600_000),
  );
  expect(state.retryAt).toBe(600_500);
  const stopped = checkDesktopBudget(state, 300_000);
  const reset = newDesktopReconnect(stopped.retryAt);
  expect(reset.expiresAt).toBeUndefined();
  expect(reset.terminal).toBeUndefined();
  expect(reset.retryAt).toBe(600_500);
});
