import { useCallback, useEffect, useRef, useState } from 'react';
import { ApiError, type UsageMetricReport } from '@chronoai/cma-sdk';
import { metricQuery, type MetricsClient, type Selection } from './model';
import { metricsFailure, type MetricsFailure } from './failure';
import { usageRequest } from './request';

export function useUsageVisible(active = true): boolean {
  const [visible, setVisible] = useState(() => document.visibilityState !== 'hidden');
  useEffect(() => {
    const changed = () => setVisible(document.visibilityState !== 'hidden');
    document.addEventListener('visibilitychange', changed);
    return () => document.removeEventListener('visibilitychange', changed);
  }, []);
  return active && visible;
}

function delay(milliseconds: number, signal: AbortSignal): Promise<void> {
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

interface State {
  readonly api: MetricsClient;
  readonly key: string;
  readonly report?: UsageMetricReport;
  readonly reportKey?: string;
  readonly pending?: UsageMetricReport;
  readonly error?: MetricsFailure;
  readonly busy: boolean;
}

/** One snapshot for summary, charts, pages and detail; opening never changes a workload. */
export function useUsageReport(
  api: MetricsClient,
  selection: Selection,
  admin: boolean,
  active = true,
) {
  const visible = useUsageVisible(active);
  const [revision, setRevision] = useState(0);
  const [state, setState] = useState<State>();
  const busy = useRef(false);
  const retryAt = useRef(0);
  const lastStarted = useRef(0);
  const pendingRequest = useRef<
    { api: MetricsClient; key: string; revision: number; report: UsageMetricReport } | undefined
  >(undefined);
  const completed = useRef<{ api: MetricsClient; key: string; revision: number } | undefined>(
    undefined,
  );
  // Sorting only changes a page of the existing report.
  const key = JSON.stringify({ ...selection, sort: undefined, order: undefined });
  const refresh = useCallback(() => {
    if (!busy.current && Date.now() >= retryAt.current) setRevision((value) => value + 1);
  }, []);

  useEffect(() => {
    if (!visible) return;
    const prior = completed.current;
    if (
      prior?.api === api &&
      prior.key === key &&
      prior.revision === revision &&
      Date.now() < Math.max(lastStarted.current + 60000, retryAt.current)
    )
      return;
    const controller = new AbortController();
    const signal = controller.signal;
    const run = async () => {
      busy.current = true;
      lastStarted.current = Date.now();
      completed.current = undefined;
      setState((previous) => ({
        api,
        key,
        report: previous?.api === api ? previous.report : undefined,
        reportKey: previous?.api === api ? previous.reportKey : undefined,
        busy: true,
      }));
      try {
        const input = JSON.parse(key) as Selection;
        const pending = pendingRequest.current;
        let report =
          pending?.api === api && pending.key === key && pending.revision === revision
            ? pending.report
            : await usageRequest(api, signal, () =>
                api.create(metricQuery(input, admin), { signal }),
              );
        while (!signal.aborted && report.status === 'pending') {
          pendingRequest.current = { api, key, revision, report };
          setState((previous) => ({
            api,
            key,
            report: previous?.api === api ? previous.report : undefined,
            reportKey: previous?.api === api ? previous.reportKey : undefined,
            pending: report,
            busy: true,
          }));
          await delay(
            Math.min(60000, Math.max(1000, Number(report.retry_after_seconds) * 1000)),
            signal,
          );
          if (Date.now() / 1000 >= Number(report.expires_at)) throw new ApiError(410, 'Expired');
          report = await usageRequest(api, signal, () => api.report(report.report_id, { signal }));
        }
        if (signal.aborted) return;
        pendingRequest.current = undefined;
        if (report.status !== 'ready' || !report.summary) throw new Error('Report failed');
        retryAt.current = 0;
        setState({ api, key, report, reportKey: key, busy: false });
      } catch (error) {
        if (signal.aborted) return;
        pendingRequest.current = undefined;
        const failure = metricsFailure(error);
        retryAt.current = failure.retryAt ?? 0;
        setState((previous) => ({
          api,
          key,
          report:
            failure.denied || failure.expired || previous?.api !== api
              ? undefined
              : previous.report,
          reportKey: previous?.reportKey,
          error: failure,
          busy: false,
        }));
      } finally {
        if (!signal.aborted) {
          busy.current = false;
          completed.current = { api, key, revision };
        }
      }
    };
    void run();
    return () => {
      controller.abort();
      busy.current = false;
    };
  }, [api, key, admin, visible, revision]);

  useEffect(() => {
    if (!visible) return;
    const timer = setInterval(() => {
      if (Date.now() - lastStarted.current >= 60000) refresh();
    }, 60000);
    return () => clearInterval(timer);
  }, [visible, refresh]);

  return {
    report: state?.api === api ? state.report : undefined,
    pending: state?.api === api && state.key === key ? state.pending : undefined,
    stale: state?.api === api && !!state.report && state.reportKey !== key,
    error: state?.api === api && state.key === key ? state.error : undefined,
    busy: visible && (state?.api !== api || state.key !== key || state.busy),
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
