import { useCallback, useEffect, useState } from 'react';
import { ApiError, TransportError } from '@chronoai/cma-sdk';

export interface AuthorizedValue<Value> {
  readonly value: Value;
  readonly revalidateAt?: string | null;
}
export type AuthorizedRead<Value> =
  | { readonly status: 'loading' }
  | { readonly status: 'error'; readonly message: string; readonly transient?: true }
  | { readonly status: 'ready'; readonly value: Value };

/** Values belong to one read context and expire independently of a pending refresh. */
export function useAuthorizedRead<Value>(
  read: ((signal: AbortSignal) => Promise<AuthorizedValue<Value>>) | undefined,
) {
  const [revision, setRevision] = useState(0);
  const [stored, setStored] = useState<{
    readonly read: typeof read;
    readonly revision: number;
    readonly result: AuthorizedRead<Value>;
  }>();
  const reload = useCallback(() => setRevision((value) => value + 1), []);
  useEffect(() => {
    if (read === undefined) return;
    let active = true;
    let pending: AbortController | undefined;
    let expiration: ReturnType<typeof setTimeout> | undefined;
    const publish = (result: AuthorizedRead<Value>) => {
      if (active) setStored({ read, revision, result });
    };
    const clear = () => {
      clearTimeout(expiration);
      pending?.abort();
      pending = undefined;
      publish({ status: 'loading' });
    };
    const load = async () => {
      if (!active || pending !== undefined || document.visibilityState === 'hidden') return;
      const request = new AbortController();
      pending = request;
      const started = Date.now();
      const timeout = setTimeout(() => {
        request.abort();
        if (pending === request) {
          pending = undefined;
          clearTimeout(expiration);
          publish({
            status: 'error',
            message: 'Current resource access could not be checked. Try again.',
            transient: true,
          });
        }
      }, 15_000);
      try {
        const result = await read(request.signal);
        request.signal.throwIfAborted();
        if (!active || pending !== request) return;
        const deadline = Math.min(
          started + 55_000,
          result.revalidateAt == null ? Infinity : Date.parse(result.revalidateAt),
        );
        if (!Number.isFinite(deadline) || deadline <= Date.now())
          throw new Error('Current resource access has expired. Try again.');
        clearTimeout(expiration);
        expiration = setTimeout(() => {
          clear();
          void load();
        }, deadline - Date.now());
        publish({ status: 'ready', value: result.value });
      } catch (error) {
        if (active && pending === request) {
          clearTimeout(expiration);
          publish({
            status: 'error',
            ...((error instanceof ApiError && error.isRetryable) ||
            error instanceof TransportError ||
            error instanceof TypeError
              ? { transient: true as const }
              : {}),
            message:
              error instanceof Error && !request.signal.aborted
                ? error.message
                : 'Current resource access could not be checked. Try again.',
          });
        }
      } finally {
        clearTimeout(timeout);
        if (pending === request) pending = undefined;
      }
    };
    const foreground = () => {
      clear();
      void load();
    };
    void load();
    const refresh = setInterval(() => void load(), 30_000);
    document.addEventListener('visibilitychange', foreground);
    window.addEventListener('focus', foreground);
    return () => {
      active = false;
      pending?.abort();
      clearTimeout(expiration);
      clearInterval(refresh);
      document.removeEventListener('visibilitychange', foreground);
      window.removeEventListener('focus', foreground);
    };
  }, [read, revision]);
  const state: AuthorizedRead<Value> =
    stored !== undefined &&
    stored.read === read &&
    stored.revision === revision &&
    read !== undefined
      ? stored.result
      : { status: 'loading' };
  return { state, reload };
}
