import { rateLimitDelay } from './readRetry';
import type { V2AgentStreamFrame, ExternalApprovalProjection } from '@cma/client';
import { transientReadFailure } from '../loadAgent';
import type { AgentV2Session } from './client';
import { ReadinessWake } from '../readinessWait';

function delay(ms: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve) => {
    const finish = () => {
      clearTimeout(timer);
      signal.removeEventListener('abort', finish);
      resolve();
    };
    const timer = setTimeout(finish, ms);
    signal.addEventListener('abort', finish, { once: true });
    if (signal.aborted) finish();
  });
}

/** Reconnect read-only observation after a held lease closes; never resubmit input. */
export async function followV2Agent(
  v2: AgentV2Session,
  afterCursor: string,
  signal: AbortSignal,
  onEvent: (frame: V2AgentStreamFrame) => void,
  onFailure: (message?: string) => void,
  onExternalApprovalProjection?: (projection: ExternalApprovalProjection) => void,
): Promise<void> {
  let cursor = afterCursor;
  let retry = 250;
  let failures = 0;
  let notice: ReturnType<typeof setTimeout> | undefined;
  const visibility = new ReadinessWake();
  let attempt: AbortController | undefined;
  const onVisibility = () => {
    if (document.hidden || document.visibilityState === 'hidden') attempt?.abort();
  };
  document.addEventListener('visibilitychange', onVisibility);
  const reconnecting = () => {
    notice ??= setTimeout(() => {
      if (!signal.aborted)
        onFailure('Live updates are reconnecting. Recorded activity is retained.');
    }, 2500);
  };
  try {
    while (!signal.aborted) {
      let progressed = false;
      let failure: unknown;
      try {
        if (document.hidden) await visibility.wait(0, signal);
        signal.throwIfAborted();
        attempt = new AbortController();
        const observing = AbortSignal.any([signal, attempt.signal]);
        await v2.authorize(observing);
        await v2.events.events(v2.journal.agentId, {
          externalApprovals: true,
          onExternalApprovalProjection,
          afterCursor: cursor,
          messageBindings: true,
          toolOutput: true,
          retainedOutput: true,
          liveTimestamps: true,
          signal: observing,
          onEvent(frame) {
            observing.throwIfAborted();
            v2.journal.read();
            onEvent(frame);
            clearTimeout(notice);
            notice = undefined;
            onFailure(undefined);
          },
          onCursor(next) {
            progressed ||= next !== cursor;
            cursor = next;
          },
        });
        reconnecting();
      } catch (error) {
        if (signal.aborted) return;
        if (!attempt?.signal.aborted && !transientReadFailure(error)) throw error;
        failure = error;
        reconnecting();
      }
      failures = progressed ? 1 : failures + 1;
      retry = progressed ? 250 : Math.min(retry * 2, 10000);
      await delay(rateLimitDelay(failure, retry * (0.5 + Math.random() * 0.5), failures), signal);
    }
  } finally {
    clearTimeout(notice);
    attempt?.abort();
    document.removeEventListener('visibilitychange', onVisibility);
  }
}
