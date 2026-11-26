import { ApiError } from '@cma/client';
import { readRetryDelay } from './transport/readRetry';

// Allow runtime recovery across several short-lived streams, but never retry forever.
export const desktopRetryBudgetMs = 5 * 60_000;
export const desktopStableMs = 60_000;
export const desktopStopMessages = {
  unavailable: 'Desktop is currently unavailable. Reconnect when it becomes available.',
  authorization: 'Desktop access was denied. Check your access before reconnecting.',
  refused: 'Desktop connection refused.',
  exhausted: 'Could not reconnect automatically. Choose Reconnect to try again.',
};
export type DesktopStop = keyof typeof desktopStopMessages;

export interface DesktopReconnect {
  started: boolean;
  automaticAttempts: number;
  failures: number;
  retryAt: number;
  expiresAt?: number;
  terminal?: DesktopStop;
}

export function newDesktopReconnect(retryAt = 0, started = false): DesktopReconnect {
  return { started, automaticAttempts: 0, failures: 0, retryAt };
}

export function checkDesktopBudget(state: DesktopReconnect, now: number): DesktopReconnect {
  if (state.terminal) return state;
  return state.expiresAt !== undefined && now >= state.expiresAt
    ? { ...state, terminal: 'exhausted' }
    : state;
}

/** Called only when a visible, idle viewer actually begins an attempt. */
export function startDesktopAttempt(state: DesktopReconnect, now: number): DesktopReconnect {
  state = checkDesktopBudget(state, now);
  if (state.terminal) return state;
  return {
    ...state,
    started: true,
    automaticAttempts: state.automaticAttempts + (state.started ? 1 : 0),
  };
}

export function failDesktopAttempt(
  state: DesktopReconnect,
  now: number,
  cause?: unknown,
  refused = false,
): DesktopReconnect {
  if (state.terminal) return state;
  if (refused) return { ...state, terminal: 'refused' };
  if (cause instanceof ApiError) {
    if (cause.status === 404) return { ...state, terminal: 'unavailable' };
    if (cause.status === 401 || cause.status === 403)
      return { ...state, terminal: 'authorization' };
  }
  const failures = state.failures + 1;
  return checkDesktopBudget(
    {
      ...state,
      failures,
      expiresAt: state.expiresAt ?? now + desktopRetryBudgetMs,
      // Even an exhausted attempt can impose a floor on the next manual retry.
      retryAt: Math.max(state.retryAt, now + readRetryDelay(cause, failures)),
    },
    now,
  );
}
