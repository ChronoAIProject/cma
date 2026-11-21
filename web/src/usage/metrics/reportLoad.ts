import { ApiError, ProtocolError, type UsageMetricReport } from '@cma/client';
import { metricQuery, type MetricsClient, type Selection } from './model';
import { metricsFailure } from './failure';
import {
  latestQuery,
  unsupportedLatestRoute,
  type LatestMetricsClient,
  type LatestUsageMetricView,
} from './latest';
import { jitteredRetryDelay } from './request';
import {
  retryUsageRead,
  waitForUsageRetry as waitForRetry,
  REPORT_OBSERVATION_MS,
  UsageObservationLimit,
} from './retry';
import { reportRequest } from './reportRequests';
import {
  abortWhenIdle,
  clearReportOwner,
  installVisibilityGuard,
  liveEntries,
  notify,
  type ReportEntry,
} from './reportCache';

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
  entry.refreshReportId = view.refresh.report_id ?? undefined;
  if (!entry.latestPending) entry.pollAt = 0;
  entry.pending = undefined;
  entry.error = undefined;
  entry.retryAt = 0;
  entry.completedAt = Date.now();
  if (view.refresh.state !== 'pending') entry.observationStartedAt = 0;
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
  once = false,
) {
  const latest = api.latest;
  // The retained view contract supports the report windows that can be
  // retained server-side. Keep the existing report API for the one-hour view.
  if (!latest || selection.range === '1h') return false;
  // After admission observe the exact job. Repeating latest while it fails can
  // create another job and hide generation failures behind an endless 202 loop.
  if (entry.latestPending && entry.refreshReportId && !once)
    await observePending(api, entry, signal, principal, admin);
  let view = await observeReport(api, entry, signal, (requestSignal) =>
    latest.call(api, latestQuery(selection, admin, forceRefresh && !entry.latestPending), {
      signal: requestSignal,
    }),
  );
  while (!signal.aborted) {
    if (view.report.status === 'ready' && view.report.summary) {
      commitLatest(entry, view, principal, admin);
      if (view.refresh.state !== 'pending') return true;
    } else {
      if (view.report.status !== 'pending') throw new ProtocolError('Report failed');
      validateReport(view.report, principal, admin);
      entry.pending = view.report;
      entry.latestPending = true;
      entry.refreshReportId = view.report.report_id;
    }
    if (!entry.refreshReportId) throw new ProtocolError('Missing refresh report identity');
    entry.refreshing = true;
    entry.pollAt = Date.now() + jitteredRetryDelay(view.refresh.retry_after_seconds * 1000);
    notify(entry);
    if (once && entry.visibleSubscribers === 0) {
      entry.observationStartedAt = 0;
      return true;
    }
    await observePending(api, entry, signal, principal, admin);
    view = await observeReport(api, entry, signal, (requestSignal) =>
      latest.call(api, latestQuery(selection, admin, false), { signal: requestSignal }),
    );
  }
  return true;
}

async function observePending(
  api: MetricsClient,
  entry: ReportEntry,
  signal: AbortSignal,
  principal: string | undefined,
  admin: boolean,
) {
  const id = entry.refreshReportId!;
  while (true) {
    if (entry.pollAt > Date.now()) await waitForRetry(entry.pollAt - Date.now(), signal);
    const report = await observeReport(api, entry, signal, (requestSignal) =>
      api.report(id, { signal: requestSignal }),
    );
    signal.throwIfAborted();
    validateReport(report, principal, admin);
    if (report.report_id !== id) throw new ProtocolError('Report identity changed');
    if (Date.now() / 1000 >= Number(report.expires_at)) throw new ApiError(410, 'Expired');
    if (report.status === 'ready' && report.summary) return;
    if (report.status !== 'pending') throw new ApiError(503, 'Report generation failed');
    if (!entry.report) entry.pending = report;
    entry.pollAt = Date.now() + retryAfterMilliseconds(report);
    notify(entry);
  }
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
    report = await observeReport(api, entry, signal, (requestSignal) => {
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
    report = await observeReport(api, entry, signal, (requestSignal) =>
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
  entry.observationStartedAt = 0;
  notify(entry);
}

export function startReportLoad(
  api: MetricsClient,
  selection: Selection,
  admin: boolean,
  owner: string,
  entry: ReportEntry,
  keepAlive: boolean,
  forceRefresh: boolean,
  principal: string | undefined,
  background = false,
) {
  if (entry.authorityBlocked || entry.stopped) return () => {};
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
  entry.background = background;
  if (!entry.observationStartedAt) entry.observationStartedAt = Date.now();
  const remaining = entry.observationStartedAt + REPORT_OBSERVATION_MS - Date.now();
  if (remaining <= 0) controller.abort(new UsageObservationLimit());
  const limit = setTimeout(
    () => controller.abort(new UsageObservationLimit()),
    Math.max(0, remaining),
  );
  entry.error = undefined;
  entry.refreshing = entry.report !== undefined;
  entry.lastStarted = Date.now();
  notify(entry);
  const run = (async () => {
    signal.throwIfAborted();
    // A deliberate retry may start a replacement, but never erase a server floor.
    if (entry.pollAt > Date.now()) await waitForRetry(entry.pollAt - Date.now(), signal);
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
        background,
      );
    } catch (error) {
      if (background || !unsupportedLatestRoute(error)) throw error;
    }
    signal.throwIfAborted();
    if (!usedLatest) await loadLegacy(api, selection, admin, entry, signal, principal);
  })();
  entry.inFlight = run;
  void run
    .catch((error: unknown) => {
      if (signal.aborted && !(signal.reason instanceof UsageObservationLimit)) return;
      if (background && unsupportedLatestRoute(error)) {
        entry.error = undefined;
        entry.observationStartedAt = 0;
        return;
      }
      const failure = metricsFailure(signal.aborted ? signal.reason : error);
      entry.stopped = !background;
      entry.retrying = false;
      entry.retryAt = Math.max(failure.retryAt ?? 0, entry.retry.retryAt);
      entry.refreshing = false;
      if (failure.denied || failure.expired) {
        clearReportOwner(api, owner, failure);
        entry.report = undefined;
      }
      entry.error = { ...failure, retryAt: entry.retryAt || undefined };
      notify(entry);
    })
    .finally(() => {
      clearTimeout(limit);
      if (entry.inFlight !== run) {
        release();
        return;
      }
      if (background && entry.visibleSubscribers === 0) entry.observationStartedAt = 0;
      entry.inFlight = undefined;
      entry.controller = undefined;
      liveEntries.delete(entry);
      entry.refreshing = false;
      release();
      notify(entry);
    });
  return release;
}

function observeReport<T>(
  api: MetricsClient,
  entry: ReportEntry,
  signal: AbortSignal,
  load: (signal: AbortSignal) => Promise<T>,
) {
  return retryUsageRead(
    api,
    signal,
    (requestSignal) =>
      reportRequest(
        api,
        requestSignal,
        () => load(requestSignal),
        entry.background && entry.visibleSubscribers === 0,
      ),
    entry.retry,
    (waiting) => {
      entry.retrying = waiting;
      entry.retryAt = entry.retry.retryAt;
      notify(entry);
    },
    entry.background && entry.visibleSubscribers === 0 ? 0 : 3,
  );
}
