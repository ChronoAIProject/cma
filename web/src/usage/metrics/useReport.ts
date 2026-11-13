import { useCallback, useContext, useEffect, useMemo, useState } from 'react';
import { ApiError, type UsageMetricReport } from '@chronoai/cma-sdk';
import { metricQuery, type MetricsClient, type Selection } from './model';
import { metricsFailure, type MetricsFailure } from './failure';
import {
  latestQuery,
  unsupportedLatestRoute,
  type LatestMetricsClient,
  type LatestUsageMetricView,
} from './latest';
import { jitteredRetryDelay, usageRequest } from './request';
import {
  allowReportRetry,
  abortWhenIdle,
  bindCacheOwner,
  clearReportOwner,
  entryFor,
  installVisibilityGuard,
  liveEntries,
  notify,
  reportKey,
  retainVisible,
  type ReportEntry,
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

function waitForRetry(milliseconds: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    const aborted = () => {
      clearTimeout(timer);
      reject(signal.reason);
    };
    const timer = setTimeout(() => {
      signal.removeEventListener('abort', aborted);
      resolve();
    }, milliseconds);
    signal.addEventListener('abort', aborted, { once: true });
    if (signal.aborted) aborted();
  });
}

function retryAfterMilliseconds(report: UsageMetricReport): number {
  const seconds = Number(report.retry_after_seconds);
  return jitteredRetryDelay(Number.isFinite(seconds) ? seconds * 1000 : 1000);
}

function validateReport(report: UsageMetricReport, principal: string | undefined, admin: boolean) {
  if (
    report.scope.admin !== admin ||
    (principal !== undefined && report.scope.principal !== principal)
  )
    throw new ApiError(403, 'Report authority mismatch');
}

function commitLatest(
  entry: ReportEntry,
  view: LatestUsageMetricView,
  principal: string | undefined,
  admin: boolean,
) {
  if (view.report.status !== 'ready' || !view.report.summary) throw new Error('Report failed');
  validateReport(view.report, principal, admin);
  entry.report = view.report;
  entry.generatedAt = view.generated_at ?? undefined;
  entry.stale = view.stale || view.refresh.state !== 'idle';
  entry.latestPending = view.refresh.state === 'pending';
  if (!entry.latestPending) entry.pollAt = 0;
  entry.pending = undefined;
  entry.error = undefined;
  entry.retryAt = 0;
  entry.completedAt = Date.now();
  notify(entry);
}

async function loadLatest(
  api: LatestMetricsClient,
  selection: Selection,
  admin: boolean,
  entry: ReportEntry,
  signal: AbortSignal,
  principal: string | undefined,
  forceRefresh: boolean,
) {
  const latest = api.latest;
  // The retained view contract supports the report windows that can be
  // retained server-side. Keep the existing report API for the one-hour view.
  if (!latest || selection.range === '1h') return false;
  if (entry.pollAt > Date.now()) await waitForRetry(entry.pollAt - Date.now(), signal);
  signal.throwIfAborted();
  let view = await usageRequest(api, signal, (requestSignal) =>
    latest.call(api, latestQuery(selection, admin, forceRefresh && !entry.latestPending), {
      signal: requestSignal,
    }),
  );
  while (!signal.aborted) {
    if (view.report.status === 'ready' && view.report.summary) {
      commitLatest(entry, view, principal, admin);
      if (view.refresh.state !== 'pending') return true;
      entry.refreshing = true;
      notify(entry);
      entry.pollAt = Date.now() + jitteredRetryDelay(view.refresh.retry_after_seconds * 1000);
      await waitForRetry(entry.pollAt - Date.now(), signal);
      view = await usageRequest(api, signal, (requestSignal) =>
        latest.call(api, latestQuery(selection, admin, false), { signal: requestSignal }),
      );
      continue;
    }
    if (view.report.status !== 'pending') throw new Error('Report failed');
    validateReport(view.report, principal, admin);
    entry.latestPending = true;
    entry.pending = view.report;
    entry.refreshing = true;
    notify(entry);
    entry.pollAt = Date.now() + retryAfterMilliseconds(view.report);
    await waitForRetry(entry.pollAt - Date.now(), signal);
    view = await usageRequest(api, signal, (requestSignal) =>
      latest.call(api, latestQuery(selection, admin, false), { signal: requestSignal }),
    );
  }
  return true;
}

async function loadLegacy(
  api: MetricsClient,
  selection: Selection,
  admin: boolean,
  entry: ReportEntry,
  signal: AbortSignal,
  principal: string | undefined,
) {
  let report = entry.pending;
  if (!report) {
    report = await usageRequest(api, signal, (requestSignal) => {
      return api.create(metricQuery(selection, admin), { signal: requestSignal });
    });
  }
  signal.throwIfAborted();
  while (report.status === 'pending') {
    validateReport(report, principal, admin);
    if (entry.pending !== report || !entry.pollAt)
      entry.pollAt = Date.now() + retryAfterMilliseconds(report);
    entry.pending = report;
    entry.refreshing = true;
    notify(entry);
    if (entry.pollAt > Date.now()) await waitForRetry(entry.pollAt - Date.now(), signal);
    signal.throwIfAborted();
    if (Date.now() / 1000 >= Number(report.expires_at)) throw new ApiError(410, 'Expired');
    report = await usageRequest(api, signal, (requestSignal) =>
      api.report(report!.report_id, { signal: requestSignal }),
    );
    signal.throwIfAborted();
  }
  if (signal.aborted) return;
  if (report.status !== 'ready' || !report.summary) throw new Error('Report failed');
  validateReport(report, principal, admin);
  entry.report = report;
  entry.generatedAt = undefined;
  entry.latestPending = false;
  entry.pollAt = 0;
  entry.stale = false;
  entry.pending = undefined;
  entry.error = undefined;
  entry.retryAt = 0;
  entry.completedAt = Date.now();
  notify(entry);
}

function startReportLoad(
  api: MetricsClient,
  selection: Selection,
  admin: boolean,
  owner: string,
  entry: ReportEntry,
  keepAlive: boolean,
  forceRefresh: boolean,
  principal: string | undefined,
) {
  if (entry.authorityBlocked || entry.retryAt > Date.now()) return () => {};
  let released = false;
  const release = () => {
    if (released) return;
    released = true;
    if (keepAlive) entry.prefetchers = Math.max(0, entry.prefetchers - 1);
    abortWhenIdle(entry);
  };
  if (entry.inFlight) {
    if (keepAlive) entry.prefetchers += 1;
    return release;
  }
  installVisibilityGuard();
  if (keepAlive) entry.prefetchers += 1;
  const controller = new AbortController();
  const signal = controller.signal;
  entry.controller = controller;
  liveEntries.add(entry);
  entry.attempted = true;
  entry.error = undefined;
  entry.refreshing = entry.report !== undefined;
  entry.lastStarted = Date.now();
  notify(entry);
  const run = (async () => {
    const latestClient = api as LatestMetricsClient;
    let usedLatest = false;
    try {
      usedLatest = await loadLatest(
        latestClient,
        selection,
        admin,
        entry,
        signal,
        principal,
        forceRefresh,
      );
    } catch (error) {
      if (!unsupportedLatestRoute(error)) throw error;
    }
    signal.throwIfAborted();
    if (!usedLatest) await loadLegacy(api, selection, admin, entry, signal, principal);
  })();
  entry.inFlight = run;
  void run
    .catch((error: unknown) => {
      if (signal.aborted) return;
      const failure = metricsFailure(error);
      entry.retryAt = failure.retryAt ?? 0;
      entry.pending = undefined;
      entry.latestPending = false;
      entry.pollAt = 0;
      entry.refreshing = false;
      if (failure.denied || failure.expired) {
        clearReportOwner(api, owner, failure);
        entry.report = undefined;
      }
      entry.error = failure;
      notify(entry);
    })
    .finally(() => {
      if (entry.inFlight !== run) {
        release();
        return;
      }
      entry.inFlight = undefined;
      entry.controller = undefined;
      liveEntries.delete(entry);
      entry.refreshing = false;
      release();
      notify(entry);
    });
  return release;
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
  return startReportLoad(api, selection, admin, scope.cacheKey, entry, true, true, scope.principal);
}

/** One snapshot for summary, charts, pages and detail; opening never changes a workload. */
export function useUsageReport(
  api: MetricsClient,
  selection: Selection,
  admin: boolean,
  active = true,
  owner?: UsageReportOwner,
  forceOnMount = false,
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
    if (!visible || entry.inFlight || entry.authorityBlocked || entry.retryAt > Date.now()) return;
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
      revision > 0 || !entry.report,
      scope.principal,
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
    visible,
  ]);

  useEffect(() => {
    if (!visible) return;
    const timer = setInterval(() => {
      if (!entry.authorityBlocked && Date.now() - entry.lastStarted >= 60000) refresh();
    }, 60000);
    return () => clearInterval(timer);
  }, [entry, refresh, visible]);

  const report = entry.report;
  const busy = visible && !report && (entry.inFlight !== undefined || !entry.attempted);
  return {
    report,
    generatedAt: entry.generatedAt,
    pending: entry.pending,
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
    void usageRequest(scope, controller.signal, load).then(
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
