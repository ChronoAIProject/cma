import { useCallback, useEffect, useRef, useState } from 'react';
import { ApiError, TransportError } from '@cma/client';
import { readRetryDelay } from '../agent/transport/readRetry';
import { readableError } from '../vocabulary/errors';

export interface AuthorizedValue<Value> {
  readonly value: Value;
  readonly revalidateAt?: string | null;
  readonly refreshAt?: string | null;
}
export type AuthorizedRead<Value> =
  | { readonly status: 'loading' }
  | {
      readonly status: 'error';
      readonly message: string;
      readonly transient?: true;
      readonly retryAt?: number;
    }
  | {
      readonly status: 'ready';
      readonly value: Value;
      readonly stale?: boolean;
      readonly refreshError?: string;
    };

interface RetryPolicy {
  readonly scope: object;
  readonly applies: (error: ApiError) => boolean;
}

/** Values belong to one read context and expire independently of a pending refresh. */
export function useAuthorizedRead<Value>(
  read: ((signal: AbortSignal) => Promise<AuthorizedValue<Value>>) | undefined,
  retainStale = false,
  retryPolicy?: RetryPolicy,
) {
  const retryScope = retryPolicy?.scope ?? read;
  const retryWithDeadline = retryPolicy?.applies;
  const retry = useRef({ scope: retryScope, failures: 0, at: 0 });
  const retryRead = useRef<{ read: typeof read; load: () => void } | undefined>(undefined);
  const [revision, setRevision] = useState(0);
  const [stored, setStored] = useState<{
    readonly read: typeof read;
    readonly revision: number;
    readonly result: AuthorizedRead<Value>;
    readonly refreshing: boolean;
  }>();
  const reload = useCallback(() => {
    const scheduled = retryRead.current;
    if (scheduled && scheduled.read === read && retry.current.at > 0) {
      scheduled.load();
      return;
    }
    setRevision((value) => value + 1);
  }, [read]);
  useEffect(() => {
    if (read === undefined) return;
    if (retry.current.scope !== retryScope)
      retry.current = { scope: retryScope, failures: 0, at: 0 };
    let active = true;
    let retryTimer: ReturnType<typeof setTimeout> | undefined;
    let observationRefresh: ReturnType<typeof setTimeout> | undefined;
    let pending: AbortController | undefined;
    let expiration: ReturnType<typeof setTimeout> | undefined;
    // Wall-clock authority deadline of the published value. Background tabs may
    // throttle `expiration`, so foreground checks this directly.
    let validUntil: number | undefined;
    const publish = (result: AuthorizedRead<Value>) => {
      if (active)
        setStored((current) => {
          if (
            retainStale &&
            current?.read === read &&
            current.result.status === 'ready' &&
            (result.status === 'loading' || (result.status === 'error' && result.transient))
          ) {
            return {
              read,
              revision,
              result: {
                ...current.result,
                stale: true,
                refreshError:
                  result.status === 'error' ? result.message : current.result.refreshError,
              },
              refreshing: result.status === 'loading',
            };
          }
          return { read, revision, result, refreshing: false };
        });
    };
    const clear = () => {
      clearTimeout(expiration);
      validUntil = undefined;
      pending?.abort();
      pending = undefined;
      publish({ status: 'loading' });
    };
    const load = async () => {
      if (!active || pending !== undefined || document.visibilityState === 'hidden') return;
      clearTimeout(retryTimer);
      clearTimeout(observationRefresh);
      if (retry.current.at > Date.now()) {
        retryTimer = setTimeout(() => void load(), retry.current.at - Date.now());
        return;
      }
      const request = new AbortController();
      pending = request;
      setStored((current) =>
        current?.read === read && current.revision === revision && current.result.status === 'ready'
          ? { ...current, refreshing: true }
          : current,
      );
      const started = Date.now();
      const timeout = setTimeout(() => {
        request.abort();
        if (pending === request) {
          pending = undefined;
          clearTimeout(expiration);
          validUntil = undefined;
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
        validUntil = deadline;
        expiration = setTimeout(() => {
          clear();
          void load();
        }, deadline - Date.now());
        if (result.refreshAt != null) {
          // The observation can refresh while the current access remains valid.
          const delay = Math.max(250, Date.parse(result.refreshAt) - Date.now());
          if (Date.now() + delay < deadline)
            observationRefresh = setTimeout(() => void load(), delay);
        }
        retry.current = { scope: retryScope, failures: 0, at: 0 };
        publish({ status: 'ready', value: result.value });
      } catch (error) {
        if (active && pending === request) {
          clearTimeout(expiration);
          validUntil = undefined;
          const transient =
            (error instanceof ApiError && error.isRetryable) ||
            error instanceof TransportError ||
            error instanceof TypeError;
          const limited =
            error instanceof ApiError && (error.status === 429 || retryWithDeadline?.(error));
          if (limited) {
            retry.current.at = Date.now() + readRetryDelay(error, ++retry.current.failures);
            retryTimer = setTimeout(() => void load(), retry.current.at - Date.now());
          } else {
            retry.current.at = 0;
          }
          publish({
            status: 'error',
            ...(limited ? { retryAt: retry.current.at } : {}),
            ...(transient ? { transient: true as const } : {}),
            message: request.signal.aborted
              ? 'Current resource access could not be checked. Try again.'
              : readableError(error, 'Current resource access could not be checked. Try again.'),
          });
        }
      } finally {
        clearTimeout(timeout);
        if (pending === request) pending = undefined;
      }
    };
    // A value still inside its authority deadline stays visible while it is
    // revalidated, exactly as during the periodic refresh. Only a value whose
    // deadline passed while the tab was in the background is withdrawn first.
    // `focus` and `visibilitychange` usually fire together; `load` coalesces
    // them into one request instead of aborting and restarting it.
    const foreground = () => {
      if (document.visibilityState === 'hidden') {
        clearTimeout(retryTimer);
        return;
      }
      if (validUntil !== undefined && Date.now() >= validUntil) clear();
      void load();
    };
    retryRead.current = { read, load: () => void load() };
    void load();
    const refresh = setInterval(() => void load(), 30_000);
    document.addEventListener('visibilitychange', foreground);
    window.addEventListener('focus', foreground);
    return () => {
      active = false;
      retryRead.current = undefined;
      pending?.abort();
      clearTimeout(expiration);
      clearInterval(refresh);
      clearTimeout(retryTimer);
      clearTimeout(observationRefresh);
      document.removeEventListener('visibilitychange', foreground);
      window.removeEventListener('focus', foreground);
    };
  }, [read, revision, retainStale, retryScope, retryWithDeadline]);
  const state: AuthorizedRead<Value> =
    stored !== undefined &&
    stored.read === read &&
    (stored.revision === revision ||
      retainStale ||
      (stored.result.status === 'error' && stored.result.retryAt !== undefined)) &&
    read !== undefined
      ? stored.revision !== revision && stored.result.status === 'ready'
        ? { ...stored.result, stale: true }
        : stored.result
      : { status: 'loading' };
  return { state, reload, refreshing: state.status === 'ready' && stored?.refreshing === true };
}
