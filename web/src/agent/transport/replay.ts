import type { V2AgentStreamFrame } from '@chronoai/cma-sdk';
import { transientReadFailure } from '../loadAgent';
import type { AgentV2Session } from './client';

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
): Promise<void> {
  let cursor = afterCursor;
  let retry = 250;
  let notice: ReturnType<typeof setTimeout> | undefined;
  const reconnecting = () => {
    notice ??= setTimeout(() => {
      if (!signal.aborted)
        onFailure('Live updates are reconnecting. Recorded activity is retained.');
    }, 2500);
  };
  try {
    while (!signal.aborted) {
      try {
        await v2.authorize(signal);
        await v2.events.events(v2.journal.agentId, {
          afterCursor: cursor,
          signal,
          onEvent(frame) {
            signal.throwIfAborted();
            v2.journal.read();
            onEvent(frame);
            clearTimeout(notice);
            notice = undefined;
            onFailure(undefined);
          },
          onCursor(next) {
            cursor = next;
            retry = 250;
          },
        });
        reconnecting();
      } catch (error) {
        if (signal.aborted) return;
        if (!transientReadFailure(error)) throw error;
        reconnecting();
        retry = Math.min(retry * 2, 10000);
      }
      await delay(retry, signal);
    }
  } finally {
    clearTimeout(notice);
  }
}
