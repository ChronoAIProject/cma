import { useCallback, useContext, useEffect, useMemo, useState } from 'react';
import type { MetricsClient, Selection } from './model';
import { metricsFailure, type MetricsFailure } from './failure';
import { startReportLoad } from './reportLoad';
import { retryUsageRead } from './retry';
import {
  allowReportRetry,
  abortWhenIdle,
  bindCacheOwner,
  entryFor,
  nearestCommittedReport,
  reportKey,
  retainVisible,
} from './reportCache';
import {
  UsageReportOwnerContext,
  anonymousReportOwner,
  type UsageReportOwner,
} from './reportOwner';

export function useUsageVisible(active = true): boolean {
  const [visible, setVisible] = useState(() => document.visibilityState !== 'hidden');
  useEffect(() => {
    const changed = () => setVisible(document.visibilityState !== 'hidden');
    document.addEventListener('visibilitychange', changed);
    return () => document.removeEventListener('visibilitychange', changed);
  }, []);
  return active && visible;
}

/** Warm the exact default filter key before the Operations tab is painted. */
export function prefetchUsageReport(
  api: MetricsClient,
  selection: Selection,
  admin: boolean,
  owner: UsageReportOwner = anonymousReportOwner,
) {
  if (typeof document !== 'undefined' && document.visibilityState === 'hidden') return () => {};
  const scope = owner;
  bindCacheOwner(api, scope.cacheKey);
  const key = reportKey(selection, admin, scope.cacheKey);
  const entry = entryFor(api, key, scope.cacheKey);
  if (
    entry.report &&
    Date.now() - entry.completedAt < 60000 &&
    !entry.stale &&
    !entry.latestPending
  )
    return () => {};
  return startReportLoad(
    api,
    selection,
    admin,
    scope.cacheKey,
    entry,
    true,
    false,
    scope.principal,
  );
}

/** One snapshot for summary, charts, pages and detail; opening never changes a workload. */
export function useUsageReport(
  api: MetricsClient,
  selection: Selection,
  admin: boolean,
  active = true,
  owner?: UsageReportOwner,
  forceOnMount = false,
  measured = false,
  initialEnabled = false,
) {
  const visible = useUsageVisible(active);
  const [revision, setRevision] = useState(0);
  const [, redraw] = useState(0);
  // Sorting only changes a page of the existing report.
  const inheritedOwner = useContext(UsageReportOwnerContext);
  const scope = owner ?? inheritedOwner;
  const key = useMemo(
    () => reportKey(selection, admin, scope.cacheKey),
    [selection, admin, scope.cacheKey],
  );
  const entry = entryFor(api, key, scope.cacheKey);
  const refresh = useCallback(() => {
    if (!entry.inFlight && Date.now() >= entry.retryAt) {
      allowReportRetry(entry);
      setRevision((value) => value + 1);
    }
  }, [entry]);

  useEffect(() => {
    bindCacheOwner(api, scope.cacheKey);
  }, [api, scope.cacheKey]);

  useEffect(() => {
    const listener = () => redraw((value) => value + 1);
    entry.listeners.add(listener);
    return () => {
      entry.listeners.delete(listener);
      abortWhenIdle(entry);
    };
  }, [entry]);

  useEffect(() => {
    if (!visible) return;
    return retainVisible(entry);
  }, [entry, visible]);

  useEffect(() => {
    if (!visible || entry.inFlight || entry.authorityBlocked || entry.stopped) return;
    const recentlyCompleted =
      entry.report !== undefined &&
      !entry.stale &&
      !entry.latestPending &&
      Date.now() - entry.completedAt < 60000 &&
      revision === 0;
    if (recentlyCompleted && !forceOnMount) return;
    startReportLoad(
      api,
      selection,
      admin,
      scope.cacheKey,
      entry,
      false,
      revision > 0,
      scope.principal,
      false,
      measured,
      initialEnabled,
    );
  }, [
    admin,
    api,
    entry,
    forceOnMount,
    revision,
    scope.cacheKey,
    scope.principal,
    selection,
    measured,
    initialEnabled,
    visible,
  ]);

  useEffect(() => {
    if (!visible) return;
    const timer = setInterval(() => {
      if (!entry.authorityBlocked && !entry.stopped && Date.now() - entry.lastStarted >= 60000)
        refresh();
    }, 60000);
    return () => clearInterval(timer);
  }, [entry, refresh, visible]);

  const report = entry.report;
  const busy = visible && !report && (entry.inFlight !== undefined || !entry.attempted);
  return {
    report,
    fallback:
      !report && !entry.authorityBlocked
        ? nearestCommittedReport(api, selection, admin, scope.cacheKey)
        : undefined,
    startedAt: entry.observationStartedAt,
    nextCheckAt: Math.max(entry.pollAt, entry.retryAt),
    retrying: entry.retrying,
    stopped: entry.stopped,
    generatedAt: entry.generatedAt,
    pending: entry.pending,
    initial: entry.initial,
    initialProjection: entry.initialProjection,
    refreshing: entry.refreshing,
    stale: Boolean(report && (entry.stale || entry.refreshing)),
    error: entry.error,
    busy,
    refresh,
  };
}

/** Cancellable report member read, reused for pages, charts and drilldowns. */
export function useUsageRead<T>(
  load: (signal: AbortSignal) => Promise<T>,
  active: boolean,
  scope: object,
) {
  const [revision, setRevision] = useState(0);
  const [stored, setStored] = useState<{
    load: typeof load;
    revision: number;
    data?: T;
    error?: MetricsFailure;
  }>();
  useEffect(() => {
    if (!active) return;
    const controller = new AbortController();
    void retryUsageRead(scope, controller.signal, load).then(
      (data) => {
        if (!controller.signal.aborted) setStored({ load, revision, data });
      },
      (error: unknown) => {
        if (!controller.signal.aborted) setStored({ load, revision, error: metricsFailure(error) });
      },
    );
    return () => controller.abort();
  }, [load, active, revision, scope]);
  const result = stored?.load === load && stored.revision === revision ? stored : undefined;
  return {
    data: result?.data,
    error: result?.error,
    loading: active && !result,
    retry: () => setRevision((value) => value + 1),
  };
}
