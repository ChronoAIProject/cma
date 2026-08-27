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

interface LaunchResourceState {
  readonly launchId: string;
  readonly launch: Launch | null;
  readonly loading: boolean;
  readonly failure: string | null;
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
      return next;
    } catch (cause) {
      if (!controller.signal.aborted && activeLaunchId.current === launchId) {
        setResource((current) => ({
          ...(current.launchId === launchId ? current : emptyResource(launchId)),
          failure: failureMessage(cause),
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
    let timer: number | undefined;
    let controller: AbortController | undefined;

    const schedule = () => {
      if (live) timer = window.setTimeout(poll, POLL_INTERVAL_MS);
    };
    const poll = async () => {
      controller?.abort();
      const pollController = new AbortController();
      controller = pollController;
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
            current.launchId === launchId ? { ...current, offline: false } : current,
          );
        }
      } catch (cause) {
        if (live && activeLaunchId.current === launchId && !pollController.signal.aborted) {
          setResource((current) => ({
            ...(current.launchId === launchId ? current : emptyResource(launchId)),
            failure: failureMessage(cause),
            offline: typeof navigator !== 'undefined' && !navigator.onLine,
          }));
        }
      } finally {
        if (live && activeLaunchId.current === launchId && controller === pollController) {
          setResource((current) => ({
            ...(current.launchId === launchId ? current : emptyResource(launchId)),
            loading: false,
          }));
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
      if (refreshController.current?.launchId === launchId) {
        refreshController.current.controller.abort();
        refreshController.current = null;
      }
      if (timer !== undefined) window.clearTimeout(timer);
      window.removeEventListener('online', onOnline);
      window.removeEventListener('focus', onOnline);
    };
  }, [accept, launchId]);

  const current = resource.launchId === launchId ? resource : emptyResource(launchId);
  return { ...current, refresh, accept };
}
