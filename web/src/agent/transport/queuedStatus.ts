import { observeRead } from './readObserver';
import { retrySchedule } from './readRetry';
import { useEffect, useRef, useState } from 'react';
import {
  ApiError,
  commandIsTerminal,
  type V2QueuedCancellationObservation,
} from '@chronoai/cma-sdk';
import { transientReadFailure } from '../loadAgent';
import type { AgentV2Session } from './client';
import { cancelQueuedMessage, recoverQueuedCancellation } from './queuedCancellation';

export interface QueuedMessageControl {
  readonly message?: string;
  readonly notice?: string;
  readonly error?: string;
  readonly canCancel: boolean;
  readonly needsRecovery: boolean;
  readonly busy: boolean;
  readonly cancel: () => Promise<void>;
  readonly recover: () => Promise<void>;
}

/** Observe only the latest locally accepted message; never infer a target from the active turn. */
export function useQueuedMessage(
  session: AgentV2Session | undefined,
  enabled: boolean,
  otherActionPending: boolean,
  onIdentityChange: (error: unknown) => boolean,
): QueuedMessageControl {
  const [revision, refresh] = useState(0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<{ session: AgentV2Session; message: string }>();
  const [observed, setObserved] = useState<{
    session: AgentV2Session;
    responseId: string;
    value?: V2QueuedCancellationObservation;
    error?: string;
  }>();
  const action = useRef<AbortController | undefined>(undefined);
  let input, retained;
  let storageError: string | undefined;
  try {
    input = session?.journal.read();
    retained = session?.journal.queuedCancellation.read();
  } catch (cause) {
    storageError =
      cause instanceof Error ? cause.message : 'Your cancellation could not be recovered.';
  }
  const receipt = input?.state === 'accepted' ? input.receipt : undefined;
  const responseId = receipt?.responseId;
  const commandId = receipt?.commandId;

  useEffect(() => () => action.current?.abort(), [session, enabled]);
  useEffect(() => {
    if (session === undefined || responseId === undefined || commandId === undefined || !enabled)
      return;
    let failures = 0;
    const observer = observeRead(async (signal) => {
      try {
        await session.authorize(signal);
        const value = await session.responses.observeQueuedCancellation(
          session.journal.agentId,
          responseId,
          { signal: signal },
        );
        await session.authorize(signal);
        if (value.response.receipt.command_id !== commandId)
          throw new Error('The message observation changed its original request.');
        setObserved({ session, responseId, value });
        if (commandIsTerminal(value.response)) return;
        failures = 0;
      } catch (cause) {
        if (signal.aborted || onIdentityChange(cause)) return;
        // An older server cannot offer this optional operation.
        if (cause instanceof ApiError && cause.status === 404) {
          setObserved({ session, responseId });
          return;
        }
        setObserved({ session, responseId, error: 'Your latest message status is unavailable.' });
        if (!transientReadFailure(cause)) return;
        return retrySchedule(cause, ++failures);
      }
      return { next: 1000, cooldown: 1000 };
    });
    return () => observer.dispose();
  }, [session, responseId, commandId, enabled, revision, onIdentityChange]);

  const current =
    observed?.session === session && observed?.responseId === responseId ? observed : undefined;
  const needsRecovery = retained?.commandId === null;
  const cancelled = retained?.commandId != null && retained.responseId === responseId;
  const canCancel =
    enabled &&
    !otherActionPending &&
    !busy &&
    !storageError &&
    !needsRecovery &&
    !cancelled &&
    current?.value?.cancellable === true;
  const run = async (recover: boolean) => {
    if (
      session === undefined ||
      !enabled ||
      otherActionPending ||
      action.current !== undefined ||
      storageError
    )
      return;
    if (recover ? !needsRecovery : !canCancel || receipt === undefined) return;
    const request = new AbortController();
    action.current = request;
    setBusy(true);
    setError(undefined);
    try {
      if (recover) await recoverQueuedCancellation(session, request.signal);
      else await cancelQueuedMessage(session, receipt!, crypto.randomUUID(), request.signal);
    } catch (cause) {
      if (!request.signal.aborted && !onIdentityChange(cause))
        setError({
          session,
          message:
            cause instanceof Error ? cause.message : 'Your cancellation could not be confirmed.',
        });
    } finally {
      if (action.current === request) {
        action.current = undefined;
        setBusy(false);
        refresh((value) => value + 1);
      }
    }
  };
  return {
    message: current?.value?.cancellable && !needsRecovery && !cancelled ? input?.input : undefined,
    notice: needsRecovery
      ? 'Your queued-message cancellation is awaiting confirmation.'
      : cancelled
        ? 'Your queued message was cancelled before it started.'
        : current?.value?.cancellable
          ? 'Your latest message is waiting to start.'
          : undefined,
    error:
      storageError ?? (error?.session === session ? error?.message : undefined) ?? current?.error,
    canCancel,
    needsRecovery,
    busy: busy || otherActionPending || !enabled,
    cancel: () => run(false),
    recover: () => run(true),
  };
}
