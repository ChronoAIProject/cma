import { useEffect, useMemo, useRef, useState } from 'react';
import { ApiError } from '@cma/client';
import { CommandKeys, type CmaWebClient } from '../api/client';
import {
  loadRecoveryIntent,
  retireRecoveryReceipt,
  saveRecoveryIntent,
  type RecoveryIntent as Intent,
} from './retainedRecoveryStorage';

export function useRetainedRecovery(
  client: CmaWebClient,
  sandboxId: string,
  ownerId: string,
  permitted: boolean,
  failureRevision: number | undefined,
  onAccepted: () => void,
) {
  const scope = useMemo(() => ({ client, sandboxId, ownerId }), [client, ownerId, sandboxId]);
  const [stored, setStored] = useState(() => ({
    scope,
    ...loadRecoveryIntent(ownerId, sandboxId),
  }));
  if (stored.scope !== scope) setStored({ scope, ...loadRecoveryIntent(ownerId, sandboxId) });
  const intent = stored.scope === scope ? stored.intent : undefined;
  const storageError = stored.scope === scope ? stored.error : undefined;
  const work = useRef<AbortController | undefined>(undefined);
  const [running, setRunning] = useState<AbortController>();
  useEffect(
    () => () => {
      work.current?.abort();
      work.current = undefined;
    },
    [scope, permitted],
  );

  useEffect(() => {
    if (
      storageError ||
      !intent?.receipt ||
      failureRevision === undefined ||
      failureRevision <= intent.receipt.revision
    )
      return;
    let cancelled = false;
    queueMicrotask(() => {
      if (cancelled) return;
      const next = retireRecoveryReceipt(ownerId, sandboxId, intent);
      setStored((current) => (current.scope === scope ? { scope, ...next } : current));
    });
    return () => {
      cancelled = true;
    };
  }, [failureRevision, intent, ownerId, sandboxId, scope, storageError]);

  const publish = (next: Intent) => {
    let error: string | undefined;
    try {
      saveRecoveryIntent(ownerId, sandboxId, next);
    } catch {
      error =
        'The recovery request could not be saved. Restore browser session storage before continuing.';
    }
    setStored((current) => (current.scope === scope ? { scope, intent: next, error } : current));
    return error === undefined;
  };
  const submit = async (next: Intent, retrying = false) => {
    if (!permitted || storageError || !client.recoverSandboxRetained || work.current) return;
    if (
      !publish({
        ...next,
        refused: false,
        message: 'Recovery is not yet confirmed. Retry to check this same request.',
      })
    )
      return;
    const request = new AbortController();
    work.current = request;
    setRunning(request);
    const timeout = setTimeout(() => request.abort(), 35_000);
    try {
      const receipt = await client.recoverSandboxRetained(sandboxId, next.revision, {
        idempotencyKey: next.key,
        signal: request.signal,
      });
      request.signal.throwIfAborted();
      publish({ ...next, receipt });
      onAccepted();
    } catch (error) {
      if (!request.signal.aborted) {
        const refused =
          error instanceof ApiError &&
          error.status < 500 &&
          ![408, 429].includes(error.status) &&
          // During rollback a predecessor may refuse a newer key it cannot replay.
          !(
            retrying &&
            error.status === 409 &&
            error.code === 'retained_recovery_authority_changed'
          );
        publish({
          ...next,
          refused,
          message: refused
            ? 'Recovery was not admitted. Refresh the Sandbox to review its current state. Your saved environment and conversations are retained.'
            : 'Recovery is not yet confirmed. Retry to check this same request.',
        });
      }
    } finally {
      clearTimeout(timeout);
      if (work.current === request) work.current = undefined;
      setRunning((current) => (current === request ? undefined : current));
    }
  };
  return {
    intent,
    storageError,
    busy: running !== undefined && !running.signal.aborted,
    start: (revision: number) => {
      if (intent && !intent.refused) return;
      void submit({ revision, key: new CommandKeys().for('retained-recovery') });
    },
    retry: () => {
      if (intent && !intent.receipt && !intent.refused) void submit(intent, true);
    },
  };
}
