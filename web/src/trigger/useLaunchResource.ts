import { useCallback, useEffect, useRef, useState } from 'react';
import { readLaunch, readLaunchProgress, type Launch } from '../api/client';

interface LaunchResource {
  readonly launch: Launch | null;
  readonly loading: boolean;
  readonly failure: string | null;
  readonly offline: boolean;
  readonly lastUpdated: number | null;
  readonly liveSummary: string;
  readonly refresh: () => Promise<Launch | null>;
  readonly accept: (launch: Launch) => void;
}

const POLL_INTERVAL_MS = 2500;

function failureMessage(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

/** Replay progress by cursor, then reconcile a snapshot without moving revisions backward. */
export function useLaunchResource(launchId: string): LaunchResource {
  const [launch, setLaunch] = useState<Launch | null>(null);
  const [loading, setLoading] = useState(true);
  const [failure, setFailure] = useState<string | null>(null);
  const [offline, setOffline] = useState(false);
  const [lastUpdated, setLastUpdated] = useState<number | null>(null);
  const [liveSummary, setLiveSummary] = useState('');
  const cursor = useRef(0);
  const revision = useRef(-1);

  const accept = useCallback((next: Launch) => {
    if (next.revision_number < revision.current) {
      return;
    }
    revision.current = next.revision_number;
    setLaunch(next);
    setLastUpdated(Date.now());
    setOffline(false);
    setFailure(null);
    const active = next.steps.find(
      (step) => step.status === 'running' || step.status === 'blocked',
    );
    setLiveSummary(
      active === undefined
        ? `Launch ${next.state.replaceAll('_', ' ')}`
        : active.safe_detail || active.key.replaceAll('_', ' '),
    );
  }, []);

  const refresh = useCallback(async (): Promise<Launch | null> => {
    try {
      const next = await readLaunch(launchId);
      accept(next);
      return next;
    } catch (cause) {
      setFailure(failureMessage(cause));
      setOffline(typeof navigator !== 'undefined' && !navigator.onLine);
      return null;
    } finally {
      setLoading(false);
    }
  }, [accept, launchId]);

  useEffect(() => {
    cursor.current = 0;
    revision.current = -1;
    let live = true;
    let timer: number | undefined;
    let controller: AbortController | undefined;

    const schedule = () => {
      if (live) timer = window.setTimeout(poll, POLL_INTERVAL_MS);
    };
    const poll = async () => {
      controller = new AbortController();
      try {
        const progress = await readLaunchProgress(launchId, cursor.current, controller.signal);
        if (!live) return;
        if (progress.next_cursor >= cursor.current) {
          cursor.current = progress.next_cursor;
        }
        if (progress.events.length > 0 || revision.current < 0) {
          const next = await readLaunch(launchId, controller.signal);
          if (live) accept(next);
        } else {
          setOffline(false);
        }
      } catch (cause) {
        if (live && !controller.signal.aborted) {
          setFailure(failureMessage(cause));
          setOffline(typeof navigator !== 'undefined' && !navigator.onLine);
        }
      } finally {
        if (live) {
          setLoading(false);
          schedule();
        }
      }
    };

    void poll();
    const onOnline = () => {
      if (timer !== undefined) window.clearTimeout(timer);
      void poll();
    };
    window.addEventListener('online', onOnline);
    window.addEventListener('focus', onOnline);
    return () => {
      live = false;
      controller?.abort();
      if (timer !== undefined) window.clearTimeout(timer);
      window.removeEventListener('online', onOnline);
      window.removeEventListener('focus', onOnline);
    };
  }, [accept, launchId]);

  return { launch, loading, failure, offline, lastUpdated, liveSummary, refresh, accept };
}
