import { useCallback, useEffect, useRef, useState } from 'react';
import { pendingRevocation } from '@cma/client';
import type { ResourceViewsClient } from './client';
import type { ShareTarget } from './sharing';
import { sharingDeadline } from './sharing';
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
      try {
        forgetShareChange(owner, target, change);
        setChanges((rows) => rows.filter((row) => row.shareId !== shareId));
        setError(undefined);
      } catch {
        setError(
          'The share change is confirmed, but the saved request could not be cleared. Restore session storage before another change.',
        );
        return;
      }
      const result = change.revoked ? 'Access revoked.' : 'Access change confirmed.';
      setNotice(
        unresolved > 0
          ? `${result} Previously submitted external requests may still complete.`
          : result,
      );
      refresh();
    },
    [changes, owner, target, refresh],
  );
  const send = async (proposed: RetainedShareChange) => {
    if (!owner || !allowed || active.current) return;
    const request = new AbortController();
    active.current = request;
    setBusy(true);
    setError(undefined);
    setNotice(undefined);
    let retained: RetainedShareChange | undefined;
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
      const receipt = await sendShareChange(client, target, retained, request.signal);
      request.signal.throwIfAborted();
      if (active.current !== request) return;
      if (!matchesShareChange(receipt, target, retained))
        throw new Error('The result did not confirm the saved share change.');
      forgetShareChange(owner, target, retained);
      setChanges((rows) => rows.filter((row) => row.shareId !== retained!.shareId));
      setNotice(retained.revoked ? 'Access revoked.' : 'Access change confirmed.');
      refresh();
    } catch (cause) {
      if (active.current !== request) return;
      // Abort and unknown replies leave the original request available for explicit retry.
      const operation = pendingRevocation(cause, target.kind, target.id);
      if (operation && retained) {
        const pending = { ...retained, operation };
        setChanges((rows) => [...rows.filter((row) => row.shareId !== pending.shareId), pending]);
        try {
          saveShareChange(owner, target, pending);
        } catch {
          setError(
            'The share change is pending. Its original request is saved; status recovery could not be saved. Keep this tab open.',
          );
        }
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
