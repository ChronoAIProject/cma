import { useCallback, useEffect, useState } from 'react';
import { denied, problem } from './model';

/** Visible-only, nonoverlapping public reads. Failed refreshes retain known records. */
export function useBotRead<T>(read: (signal: AbortSignal) => Promise<T>): {
  value?: T;
  error?: string;
  fresh: boolean;
  observedAt?: number;
  reload: () => void;
} {
  const [revision, setRevision] = useState(0);
  const [stored, setStored] = useState<{
    read: typeof read;
    value?: T;
    error?: string;
    fresh: boolean;
    observedAt?: number;
  }>();
  const reload = useCallback(() => setRevision((value) => value + 1), []);
  useEffect(() => {
    let active = true;
    let request: AbortController | undefined;
    let expiry: ReturnType<typeof setTimeout> | undefined;
    const load = async () => {
      if (!active || request !== undefined || document.visibilityState === 'hidden') return;
      const current = new AbortController();
      request = current;
      const timeout = setTimeout(() => current.abort(), 15_000);
      try {
        const value = await read(current.signal);
        current.signal.throwIfAborted();
        if (active) {
          setStored({ read, value, fresh: true, observedAt: Date.now() });
          clearTimeout(expiry);
          expiry = setTimeout(
            () =>
              setStored((previous) =>
                previous?.read === read ? { ...previous, fresh: false } : previous,
              ),
            10_000,
          );
        }
      } catch (error) {
        if (active)
          setStored((previous) => ({
            read,
            fresh: false,
            error: problem(error),
            value: denied(error) ? undefined : previous?.read === read ? previous.value : undefined,
            observedAt: previous?.read === read ? previous.observedAt : undefined,
          }));
      } finally {
        clearTimeout(timeout);
        if (request === current) request = undefined;
      }
    };
    const foreground = () => {
      setStored((previous) => (previous?.read === read ? { ...previous, fresh: false } : previous));
      void load();
    };
    void load();
    const timer = setInterval(() => void load(), 5_000);
    document.addEventListener('visibilitychange', foreground);
    return () => {
      active = false;
      request?.abort();
      clearInterval(timer);
      clearTimeout(expiry);
      document.removeEventListener('visibilitychange', foreground);
    };
  }, [read, revision]);
  return { ...(stored?.read === read ? stored : { fresh: false }), reload };
}
