import { useCallback, useEffect, useRef, useState } from 'react';
import {
  ApiError,
  TransportError,
  readLaunch,
  readLaunchProgress,
  type Launch,
} from '../api/client';

export type LaunchResourceFailureDisposition = 'retryable' | 'terminal';

interface LaunchResource {
  readonly launch: Launch | null;
  readonly loading: boolean;
  readonly failure: string | null;
  readonly failureDisposition: LaunchResourceFailureDisposition | null;
  readonly nextRetryAt: number | null;
  readonly offline: boolean;
  readonly lastUpdated: number | null;
  readonly liveSummary: string;
  readonly refresh: () => Promise<Launch | null>;
  readonly accept: (launch: Launch) => void;
}

const POLL_INTERVAL_MS = 2500;
const POLL_RETRY_DELAYS_MS = [2_500, 5_000, 10_000, 30_000, 60_000] as const;

function failureMessage(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

function failureDisposition(cause: unknown): LaunchResourceFailureDisposition {
  if (cause instanceof TransportError) return 'retryable';
  if (cause instanceof ApiError) return cause.isRetryable ? 'retryable' : 'terminal';
  return 'terminal';
}

function retryDelay(attempt: number): number {
  return POLL_RETRY_DELAYS_MS[Math.min(attempt - 1, POLL_RETRY_DELAYS_MS.length - 1)] ?? 60_000;
}

function automaticRetryDelay(cause: unknown, attempt: number): number | null {
  if (failureDisposition(cause) === 'terminal') return null;
  const backoff = retryDelay(attempt);
  if (cause instanceof ApiError && cause.status === 429) {
    return cause.retryAfterMs === undefined ? null : Math.max(backoff, cause.retryAfterMs);
  }
  return backoff;
}

interface LaunchResourceState {
  readonly launchId: string;
  readonly launch: Launch | null;
  readonly loading: boolean;
  readonly failure: string | null;
  readonly failureDisposition: LaunchResourceFailureDisposition | null;
  readonly nextRetryAt: number | null;
  readonly offline: boolean;
  readonly lastUpdated: number | null;
  readonly liveSummary: string;
}

function emptyResource(launchId: string): LaunchResourceState {
  return {
    launchId,
    launch: null,
    loading: true,
    failure: null,
    failureDisposition: null,
    nextRetryAt: null,
    offline: false,
    lastUpdated: null,
    liveSummary: '',
  };
}

function acceptedResource(current: LaunchResourceState, next: Launch): LaunchResourceState {
  const active = next.steps.find((step) => step.status === 'running' || step.status === 'blocked');
  return {
    ...current,
    launch: next,
    failure: null,
    failureDisposition: null,
    nextRetryAt: null,
    offline: false,
    lastUpdated: Date.now(),
    liveSummary:
      active === undefined
        ? `Launch ${next.state.replaceAll('_', ' ')}`
        : active.safe_detail || active.key.replaceAll('_', ' '),
  };
}

/** Replay progress by cursor, then reconcile a snapshot without moving revisions backward. */
export function useLaunchResource(launchId: string): LaunchResource {
  const [resource, setResource] = useState<LaunchResourceState>(() => emptyResource(launchId));
  const [pollGeneration, setPollGeneration] = useState(0);
  const activeLaunchId = useRef(launchId);
  const refreshController = useRef<{
    readonly launchId: string;
    readonly controller: AbortController;
  } | null>(null);

  const accept = useCallback(
    (next: Launch) => {
      if (activeLaunchId.current !== launchId || next.id !== launchId) return;
      setResource((current) => {
        const scoped = current.launchId === launchId ? current : emptyResource(launchId);
        if (scoped.launch !== null && next.revision_number < scoped.launch.revision_number) {
          return current;
        }
        return acceptedResource(scoped, next);
      });
    },
    [launchId],
  );

  const refresh = useCallback(async (): Promise<Launch | null> => {
    refreshController.current?.controller.abort();
    const controller = new AbortController();
    refreshController.current = { launchId, controller };
    try {
      const next = await readLaunch(launchId, controller.signal);
      if (controller.signal.aborted || activeLaunchId.current !== launchId) return null;
      accept(next);
      setPollGeneration((generation) => generation + 1);
      return next;
    } catch (cause) {
      if (!controller.signal.aborted && activeLaunchId.current === launchId) {
        const disposition = failureDisposition(cause);
        setResource((current) => ({
          ...(current.launchId === launchId ? current : emptyResource(launchId)),
          failure: failureMessage(cause),
          failureDisposition: disposition,
          nextRetryAt: null,
          offline: typeof navigator !== 'undefined' && !navigator.onLine,
        }));
      }
      return null;
    } finally {
      if (refreshController.current?.controller === controller) refreshController.current = null;
      if (activeLaunchId.current === launchId) {
        setResource((current) => ({
          ...(current.launchId === launchId ? current : emptyResource(launchId)),
          loading: false,
        }));
      }
    }
  }, [accept, launchId]);

  useEffect(() => {
    activeLaunchId.current = launchId;
    refreshController.current?.controller.abort();
    let live = true;
    let cursor = 0;
    let observedSnapshot = false;
    let consecutiveFailures = 0;
    let timer: number | undefined;
    let controller: AbortController | undefined;

    const schedule = (delay: number) => {
      if (live) timer = window.setTimeout(poll, delay);
    };
    const poll = async () => {
      controller?.abort();
      const pollController = new AbortController();
      controller = pollController;
      let nextDelay: number | null = POLL_INTERVAL_MS;
      try {
        const progress = await readLaunchProgress(launchId, cursor, pollController.signal);
        if (!live || activeLaunchId.current !== launchId || pollController.signal.aborted) return;
        const nextCursor = Math.max(cursor, progress.next_cursor);
        if (progress.events.length > 0 || !observedSnapshot) {
          const next = await readLaunch(launchId, pollController.signal);
          if (live && activeLaunchId.current === launchId && !pollController.signal.aborted) {
            accept(next);
            observedSnapshot = true;
            cursor = nextCursor;
          }
        } else {
          cursor = nextCursor;
          setResource((current) =>
            current.launchId === launchId
              ? {
                  ...current,
                  failure: null,
                  failureDisposition: null,
                  nextRetryAt: null,
                  offline: false,
                }
              : current,
          );
        }
        consecutiveFailures = 0;
      } catch (cause) {
        if (live && activeLaunchId.current === launchId && !pollController.signal.aborted) {
          const disposition = failureDisposition(cause);
          consecutiveFailures += 1;
          nextDelay = automaticRetryDelay(cause, consecutiveFailures);
          setResource((current) => ({
            ...(current.launchId === launchId ? current : emptyResource(launchId)),
            failure: failureMessage(cause),
            failureDisposition: disposition,
            nextRetryAt: nextDelay === null ? null : Date.now() + nextDelay,
            offline: typeof navigator !== 'undefined' && !navigator.onLine,
          }));
        }
      } finally {
        if (live && activeLaunchId.current === launchId && controller === pollController) {
          setResource((current) => ({
            ...(current.launchId === launchId ? current : emptyResource(launchId)),
            loading: false,
          }));
          if (nextDelay !== null) schedule(nextDelay);
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
      if (refreshController.current?.launchId === launchId) {
        refreshController.current.controller.abort();
        refreshController.current = null;
      }
      if (timer !== undefined) window.clearTimeout(timer);
      window.removeEventListener('online', onOnline);
      window.removeEventListener('focus', onOnline);
    };
  }, [accept, launchId, pollGeneration]);

  const current = resource.launchId === launchId ? resource : emptyResource(launchId);
  return { ...current, refresh, accept };
}
