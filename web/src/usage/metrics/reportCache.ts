import type { UsageInitialData, UsageInitialProjection, UsageMetricReport } from '@cma/client';
import type { MetricsClient, Selection } from './model';
import type { MetricsFailure } from './failure';
import type { UsageRetryState } from './retry';
import { clearDisabledViews } from './viewCapability';

export type CommittedUsageReport = UsageMetricReport & {
  /** Optional fields supplied by the cached-report API when available. */
  readonly generated_at?: number | string;
  readonly stale?: boolean;
};

export interface ReportEntry {
  owner: string;
  retry: UsageRetryState;
  retrying: boolean;
  observationStartedAt: number;
  stopped: boolean;
  background: boolean;
  report?: CommittedUsageReport;
  initial?: UsageInitialData;
  initialProjection?: UsageInitialProjection;
  initialPending: boolean;
  generatedAt?: number;
  stale: boolean;
  refreshing: boolean;
  pending?: UsageMetricReport;
  error?: MetricsFailure;
  retryAt: number;
  pollAt: number;
  latestPending: boolean;
  refreshReportId?: string;
  lastStarted: number;
  completedAt: number;
  attempted: boolean;
  authorityBlocked: boolean;
  inFlight?: Promise<void>;
  controller?: AbortController;
  listeners: Set<() => void>;
  visibleSubscribers: number;
  prefetchers: number;
}

const reportCache = new WeakMap<MetricsClient, Map<string, ReportEntry>>();
const cacheOwners = new WeakMap<MetricsClient, string>();
export const liveEntries = new Set<ReportEntry>();
const MAX_CACHE_ENTRIES = 32;
const CACHE_TTL_MS = 5 * 60 * 1000;
let visibilityGuardInstalled = false;

export function installVisibilityGuard() {
  if (visibilityGuardInstalled || typeof document === 'undefined') return;
  visibilityGuardInstalled = true;
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState !== 'hidden') return;
    for (const entry of liveEntries) cancelReportLoad(entry);
  });
}

export function reportKey(selection: Selection, admin: boolean, owner: string): string {
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

export function clearReportOwner(api: MetricsClient, owner: string, failure?: MetricsFailure) {
  clearDisabledViews(api, owner);
  const entries = reportCache.get(api);
  if (!entries) return;
  for (const [key, entry] of entries) {
    if (entry.owner !== owner) continue;
    entry.controller?.abort();
    entry.controller = undefined;
    entry.inFlight = undefined;
    liveEntries.delete(entry);
    entry.report = undefined;
    entry.initial = undefined;
    entry.initialProjection = undefined;
    entry.pending = undefined;
    entry.generatedAt = undefined;
    entry.stale = false;
    entry.refreshing = false;
    entry.completedAt = 0;
    entry.retrying = false;
    entry.observationStartedAt = 0;
    entry.stopped = true;
    entry.pollAt = 0;
    entry.latestPending = false;
    entry.initialPending = false;
    entry.refreshReportId = undefined;
    entry.error = failure;
    entry.authorityBlocked = true;
    notify(entry);
    if (entry.listeners.size === 0 && entry.visibleSubscribers === 0 && entry.prefetchers === 0)
      entries.delete(key);
  }
}

function evictEntries(api: MetricsClient, current?: ReportEntry) {
  const entries = reportCache.get(api);
  if (!entries) return;
  const now = Date.now();
  const candidates = [...entries].filter(
    ([, entry]) =>
      entry !== current &&
      entry.listeners.size === 0 &&
      entry.visibleSubscribers === 0 &&
      entry.prefetchers === 0 &&
      entry.inFlight === undefined,
  );
  for (const [key, entry] of candidates) {
    const lastActivity = entry.completedAt || entry.lastStarted;
    const expiredPending = entry.pending && now / 1000 >= Number(entry.pending.expires_at);
    if (expiredPending || (lastActivity > 0 && now - lastActivity > CACHE_TTL_MS))
      entries.delete(key);
  }
  if (entries.size <= MAX_CACHE_ENTRIES) return;
  candidates
    .filter(([key]) => entries.has(key))
    .sort(
      ([, left], [, right]) =>
        (left.completedAt || left.lastStarted) - (right.completedAt || right.lastStarted),
    )
    .slice(0, entries.size - MAX_CACHE_ENTRIES)
    .forEach(([key]) => entries.delete(key));
}

export function bindCacheOwner(api: MetricsClient, owner: string) {
  const previous = cacheOwners.get(api);
  if (previous !== undefined && previous !== owner) clearReportOwner(api, previous);
  cacheOwners.set(api, owner);
}

export function entryFor(api: MetricsClient, key: string, owner: string): ReportEntry {
  let entries = reportCache.get(api);
  if (!entries) {
    entries = new Map();
    reportCache.set(api, entries);
  }
  evictEntries(api);
  let entry = entries.get(key);
  if (!entry) {
    entry = {
      owner,
      retry: { failures: 0, retryAt: 0 },
      retrying: false,
      observationStartedAt: 0,
      stopped: false,
      background: false,
      stale: false,
      refreshing: false,
      retryAt: 0,
      pollAt: 0,
      latestPending: false,
      initialPending: false,
      lastStarted: 0,
      completedAt: 0,
      attempted: false,
      authorityBlocked: false,
      listeners: new Set(),
      visibleSubscribers: 0,
      prefetchers: 0,
    };
    entries.set(key, entry);
  }
  evictEntries(api, entry);
  return entry;
}

export function notify(entry: ReportEntry) {
  for (const listener of entry.listeners) listener();
}

export function abortWhenIdle(entry: ReportEntry) {
  if (entry.visibleSubscribers !== 0 || entry.prefetchers !== 0) return;
  cancelReportLoad(entry);
}

function cancelReportLoad(entry: ReportEntry) {
  entry.controller?.abort();
  entry.controller = undefined;
  entry.inFlight = undefined;
  liveEntries.delete(entry);
}

export function retainVisible(entry: ReportEntry) {
  entry.visibleSubscribers += 1;
  return () => {
    entry.visibleSubscribers = Math.max(0, entry.visibleSubscribers - 1);
    abortWhenIdle(entry);
  };
}

// A deliberate retry can reauthorize this selection; periodic/visibility work cannot.
export function allowReportRetry(entry: ReportEntry) {
  entry.authorityBlocked = false;
  if (entry.stopped) {
    entry.pending = undefined;
    entry.latestPending = false;
    entry.refreshReportId = undefined;
  }
  entry.stopped = false;
  entry.observationStartedAt = 0;
  entry.retry.failures = 0;
}

/** A preview may change only the range, never membership, authority or client. */
export function nearestCommittedReport(
  api: MetricsClient,
  selection: Selection,
  admin: boolean,
  owner: string,
) {
  if (selection.range !== '7d' && selection.range !== '30d') return undefined;
  const ranges = selection.range === '7d' ? (['24h', '30d'] as const) : (['7d', '24h'] as const);
  const entries = reportCache.get(api);
  for (const range of ranges) {
    const entry = entries?.get(reportKey({ ...selection, range }, admin, owner));
    if (!entry?.report || entry.authorityBlocked || entry.owner !== owner) continue;
    if (
      Date.now() - entry.completedAt >= CACHE_TTL_MS ||
      Number(entry.report.expires_at) * 1000 <= Date.now()
    )
      continue;
    return { report: entry.report, generatedAt: entry.generatedAt, range };
  }
  return undefined;
}
