import { useCallback, useEffect, useMemo, useState } from 'react';
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

export function useUsageVisible(active = true): boolean {
  const [visible, setVisible] = useState(() => document.visibilityState !== 'hidden');
  useEffect(() => {
    const changed = () => setVisible(document.visibilityState !== 'hidden');
    document.addEventListener('visibilitychange', changed);
    return () => document.removeEventListener('visibilitychange', changed);
  }, []);
  return active && visible;
}

export type CommittedUsageReport = UsageMetricReport & {
  /** Optional fields supplied by the cached-report API when available. */
  readonly generated_at?: number | string;
  readonly stale?: boolean;
};

interface ReportEntry {
  owner: string;
  report?: CommittedUsageReport;
  generatedAt?: number;
  stale: boolean;
  refreshing: boolean;
  pending?: UsageMetricReport;
  error?: MetricsFailure;
  retryAt: number;
  lastStarted: number;
  completedAt: number;
  attempted: boolean;
  inFlight?: Promise<void>;
  controller?: AbortController;
  listeners: Set<() => void>;
  visibleSubscribers: number;
  prefetchers: number;
}

const reportCache = new WeakMap<MetricsClient, Map<string, ReportEntry>>();
const cacheOwners = new WeakMap<MetricsClient, string>();
const liveEntries = new Set<ReportEntry>();
const MAX_CACHE_ENTRIES = 32;
const CACHE_TTL_MS = 5 * 60 * 1000;
let visibilityGuardInstalled = false;

function installVisibilityGuard() {
  if (visibilityGuardInstalled || typeof document === 'undefined') return;
  visibilityGuardInstalled = true;
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState !== 'hidden') return;
    for (const entry of liveEntries) entry.controller?.abort();
  });
}

function reportKey(selection: Selection, admin: boolean, owner: string): string {
  // Sort and order affect only the client-side page; every other field scopes
  // the immutable report and therefore stays in the cache key.
  return JSON.stringify({
    cache_owner: owner,
    admin,
    ...selection,
    sort: undefined,
    order: undefined,
  });
}

function clearReportOwner(api: MetricsClient, owner: string, keep?: ReportEntry) {
  const entries = reportCache.get(api);
  if (!entries) return;
  for (const [key, entry] of entries) {
    if (entry.owner !== owner || entry === keep) continue;
    entry.controller?.abort();
    liveEntries.delete(entry);
    entries.delete(key);
  }
  if (keep) {
    keep.controller?.abort();
    keep.controller = undefined;
    keep.inFlight = undefined;
    keep.report = undefined;
    keep.pending = undefined;
    keep.generatedAt = undefined;
    keep.stale = false;
    keep.refreshing = false;
  }
}

function evictEntries(api: MetricsClient, current: ReportEntry) {
  const entries = reportCache.get(api);
  if (!entries || entries.size <= 1) return;
  const now = Date.now();
  const candidates = [...entries].filter(
    ([, entry]) =>
      entry !== current &&
      entry.listeners.size === 0 &&
      entry.visibleSubscribers === 0 &&
      entry.prefetchers === 0 &&
      entry.inFlight === undefined &&
      entry.pending === undefined,
  );
  for (const [key, entry] of candidates) {
    if (entry.completedAt > 0 && now - entry.completedAt > CACHE_TTL_MS) entries.delete(key);
  }
  if (entries.size <= MAX_CACHE_ENTRIES) return;
  candidates
    .sort(([, left], [, right]) => left.completedAt - right.completedAt)
    .slice(0, entries.size - MAX_CACHE_ENTRIES)
    .forEach(([key]) => entries.delete(key));
}

function bindCacheOwner(api: MetricsClient, owner: string) {
  const previous = cacheOwners.get(api);
  if (previous !== undefined && previous !== owner) clearReportOwner(api, previous);
  cacheOwners.set(api, owner);
}

function entryFor(api: MetricsClient, key: string, owner: string): ReportEntry {
  let entries = reportCache.get(api);
  if (!entries) {
    entries = new Map();
    reportCache.set(api, entries);
  }
  let entry = entries.get(key);
  if (!entry) {
    entry = {
      owner,
      stale: false,
      refreshing: false,
      retryAt: 0,
      lastStarted: 0,
      completedAt: 0,
      attempted: false,
      listeners: new Set(),
      visibleSubscribers: 0,
      prefetchers: 0,
    };
    entries.set(key, entry);
  }
  evictEntries(api, entry);
  return entry;
}

function notify(entry: ReportEntry) {
  for (const listener of entry.listeners) listener();
}

function abortWhenIdle(entry: ReportEntry) {
  if (entry.visibleSubscribers !== 0 || entry.prefetchers !== 0) return;
  entry.controller?.abort();
  entry.controller = undefined;
  entry.inFlight = undefined;
  liveEntries.delete(entry);
}

function retainVisible(entry: ReportEntry) {
  entry.visibleSubscribers += 1;
  return () => {
    entry.visibleSubscribers = Math.max(0, entry.visibleSubscribers - 1);
    abortWhenIdle(entry);
  };
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

function validateReport(report: UsageMetricReport, owner: string, admin: boolean) {
  if (report.scope.admin !== admin) throw new Error('Report authority mismatch');
  if (owner !== 'anonymous' && report.scope.principal !== owner.split(':', 1)[0])
    throw new Error('Report authority mismatch');
}

function commitLatest(
  entry: ReportEntry,
  view: LatestUsageMetricView,
  owner: string,
  admin: boolean,
) {
  if (view.report.status !== 'ready' || !view.report.summary) throw new Error('Report failed');
  validateReport(view.report, owner, admin);
  entry.report = view.report as CommittedUsageReport;
  entry.generatedAt = view.generated_at ?? undefined;
  entry.stale = view.stale || view.refresh.state !== 'idle';
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
  owner: string,
  forceRefresh: boolean,
) {
  const latest = api.latest;
  // The retained view contract supports the report windows that can be
  // retained server-side. Keep the existing report API for the one-hour view.
  if (!latest || selection.range === '1h') return false;
  let view = await usageRequest(api, signal, (requestSignal) =>
    latest.call(api, latestQuery(selection, admin, forceRefresh), { signal: requestSignal }),
  );
  while (!signal.aborted) {
    if (view.report.status === 'ready' && view.report.summary) {
      commitLatest(entry, view, owner, admin);
      if (view.refresh.state !== 'pending') return true;
      entry.refreshing = true;
      notify(entry);
      await waitForRetry(jitteredRetryDelay(view.refresh.retry_after_seconds * 1000), signal);
      view = await usageRequest(api, signal, (requestSignal) =>
        latest.call(api, latestQuery(selection, admin, false), { signal: requestSignal }),
      );
      continue;
    }
    if (view.report.status !== 'pending') throw new Error('Report failed');
    entry.pending = view.report;
    entry.refreshing = true;
    notify(entry);
    await waitForRetry(retryAfterMilliseconds(view.report), signal);
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
  owner: string,
) {
  let report = entry.pending;
  if (!report) {
    report = await usageRequest(api, signal, (requestSignal) => {
      return api.create(metricQuery(selection, admin), { signal: requestSignal });
    });
  }
  while (!signal.aborted && report.status === 'pending') {
    entry.pending = report;
    entry.refreshing = true;
    notify(entry);
    await waitForRetry(retryAfterMilliseconds(report), signal);
    if (Date.now() / 1000 >= Number(report.expires_at)) throw new ApiError(410, 'Expired');
    report = await usageRequest(api, signal, (requestSignal) =>
      api.report(report!.report_id, { signal: requestSignal }),
    );
  }
  if (signal.aborted) return;
  if (report.status !== 'ready' || !report.summary) throw new Error('Report failed');
  validateReport(report, owner, admin);
  entry.report = report as CommittedUsageReport;
  entry.generatedAt = undefined;
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
) {
  if (entry.retryAt > Date.now()) return () => {};
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
        owner,
        forceRefresh,
      );
    } catch (error) {
      if (!unsupportedLatestRoute(error)) throw error;
    }
    if (!usedLatest) await loadLegacy(api, selection, admin, entry, signal, owner);
  })();
  entry.inFlight = run;
  void run
    .catch((error: unknown) => {
      if (signal.aborted) return;
      const failure = metricsFailure(error);
      entry.retryAt = failure.retryAt ?? 0;
      entry.pending = undefined;
      entry.refreshing = false;
      if (failure.denied || failure.expired) {
        clearReportOwner(api, owner, entry);
        entry.report = undefined;
      }
      entry.error = failure;
      notify(entry);
    })
    .finally(() => {
      if (entry.inFlight !== run) return;
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
  owner = 'anonymous',
) {
  if (typeof document !== 'undefined' && document.visibilityState === 'hidden') return () => {};
  bindCacheOwner(api, owner);
  const key = reportKey(selection, admin, owner);
  const entry = entryFor(api, key, owner);
  if (entry.report && Date.now() - entry.completedAt < 60000 && !entry.stale) return () => {};
  return startReportLoad(api, selection, admin, owner, entry, true, true);
}

/** One snapshot for summary, charts, pages and detail; opening never changes a workload. */
export function useUsageReport(
  api: MetricsClient,
  selection: Selection,
  admin: boolean,
  active = true,
  owner = 'anonymous',
  forceOnMount = false,
) {
  const visible = useUsageVisible(active);
  const [revision, setRevision] = useState(0);
  const [, redraw] = useState(0);
  // Sorting only changes a page of the existing report.
  const key = useMemo(() => reportKey(selection, admin, owner), [selection, admin, owner]);
  const entry = entryFor(api, key, owner);
  const refresh = useCallback(() => {
    if (!entry.inFlight && Date.now() >= entry.retryAt) setRevision((value) => value + 1);
  }, [entry]);

  useEffect(() => {
    bindCacheOwner(api, owner);
  }, [api, owner]);

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
    if (!visible || entry.inFlight || entry.retryAt > Date.now()) return;
    const recentlyCompleted =
      entry.report !== undefined && Date.now() - entry.completedAt < 60000 && revision === 0;
    if (recentlyCompleted && !forceOnMount) return;
    startReportLoad(api, selection, admin, owner, entry, false, revision > 0 || !entry.report);
  }, [admin, api, entry, forceOnMount, owner, revision, selection, visible]);

  useEffect(() => {
    if (!visible) return;
    const timer = setInterval(() => {
      if (Date.now() - entry.lastStarted >= 60000) refresh();
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
