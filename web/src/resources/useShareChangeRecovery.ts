import { useCallback, useEffect, useRef, useState } from 'react';
import { ApiError, pendingRevocation } from '@cma/client';
import type { ResourceViewsClient } from './client';
import type { ShareTarget } from './sharing';
import { sharingDeadline } from './sharing';
import { retainPendingOperation } from './revocationObservation';
import {
  forgetShareChange,
  matchesShareChange,
  readShareChanges,
  saveShareChange,
  sendShareChange,
  type RetainedShareChange,
} from './retainedShareChange';

export function useShareChangeRecovery(
  client: ResourceViewsClient,
  target: ShareTarget,
  owner: string | undefined,
  allowed: boolean,
  refresh: () => void,
) {
  const [changes, setChanges] = useState<readonly RetainedShareChange[]>(() => {
    if (!owner) return [];
    try {
      return readShareChanges(owner, target);
    } catch {
      return [];
    }
  });
  const [error, setError] = useState<string>();
  const [notice, setNotice] = useState<string>();
  const [busy, setBusy] = useState(false);
  const active = useRef<AbortController | undefined>(undefined);
  useEffect(
    () => () => {
      active.current?.abort();
      active.current = undefined;
    },
    [client, target.kind, target.id, owner],
  );
  useEffect(() => {
    if (!allowed) active.current?.abort();
  }, [allowed]);
  const complete = useCallback(
    (shareId: string, unresolved: number) => {
      const change = changes.find((row) => row.shareId === shareId);
      if (!change || !owner) return;
      setChanges((rows) => rows.filter((row) => row.shareId !== shareId));
      setError(undefined);
      const result = change.revoked ? 'Access revoked.' : 'Access change confirmed.';
      setNotice(unresolved > 0 ? undefined : result);
      refresh();
    },
    [changes, owner, refresh],
  );
  const send = async (proposed: RetainedShareChange) => {
    if (!owner || !allowed || active.current) return;
    const request = new AbortController();
    active.current = request;
    setBusy(true);
    setError(undefined);
    setNotice(undefined);
    let retained: RetainedShareChange | undefined;
    let sending = false;
    const adoptOperation = (original: RetainedShareChange, operation: string) => {
      const pending = { ...original, operation };
      setChanges((rows) => [...rows.filter((row) => row.shareId !== pending.shareId), pending]);
      try {
        retainPendingOperation({ owner, kind: target.kind, id: target.id }, pending, original);
      } catch {
        setError(
          'The original request is saved, but status recovery could not be saved. Keep this tab open and restore session storage.',
        );
      }
    };
    const timer = setTimeout(() => {
      request.abort();
      if (active.current === request) {
        active.current = undefined;
        setBusy(false);
        setError('The result could not be confirmed. Retry the saved share change.');
      }
    }, 15_000);
    try {
      const prior = readShareChanges(owner, target).find((row) => row.shareId === proposed.shareId);
      retained = prior ?? proposed;
      saveShareChange(owner, target, retained);
      setChanges((rows) => [...rows.filter((row) => row.shareId !== retained!.shareId), retained!]);
      if (retained.operation) return;
      const started = Date.now();
      const access = await client.resourceAccess.effective(target.kind, target.id, {
        signal: request.signal,
      });
      sharingDeadline(access, started);
      request.signal.throwIfAborted();
      sending = true;
      const receipt = await sendShareChange(client, target, retained, request.signal);
      sending = false;
      request.signal.throwIfAborted();
      if (active.current !== request) return;
      if (!matchesShareChange(receipt, target, retained))
        throw new Error('The result did not confirm the saved share change.');
      if (receipt.revocation) {
        adoptOperation(retained, receipt.revocation.operation);
        return;
      }
      forgetShareChange(owner, target, retained);
      setChanges((rows) => rows.filter((row) => row.shareId !== retained!.shareId));
      setNotice(retained.revoked ? 'Access revoked.' : 'Access change confirmed.');
      refresh();
    } catch (cause) {
      if (active.current !== request) return;
      // Abort and unknown replies leave the original request available for explicit retry.
      const operation = pendingRevocation(cause, target.kind, target.id);
      if (operation && retained) {
        adoptOperation(retained, operation);
      } else if (
        sending &&
        !request.signal.aborted &&
        retained &&
        cause instanceof ApiError &&
        cause.status === 409 &&
        cause.code === 'resource_access_changed'
      ) {
        try {
          forgetShareChange(owner, target, retained);
          setChanges((rows) => rows.filter((row) => row.shareId !== retained!.shareId));
        } catch {
          setError(
            'The share change was rejected, but the saved request could not be cleared. Restore session storage before another change.',
          );
          return;
        }
        setError(
          'The share change was rejected because sharing changed. Review current access before making a new change.',
        );
        refresh();
      } else
        setError(
          cause instanceof Error
            ? cause.message
            : 'The share change could not be confirmed. Retry the saved request.',
        );
    } finally {
      clearTimeout(timer);
      if (active.current === request) {
        active.current = undefined;
        setBusy(false);
      }
    }
  };
  return { changes, error, notice, busy, send, complete };
}
