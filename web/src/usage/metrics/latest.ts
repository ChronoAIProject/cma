import { ApiError, type UsageMetricReport } from '@chronoai/cma-sdk';
import { metricQuery, type MetricsClient, type Selection } from './model';

export interface LatestUsageMetricView {
  readonly report: UsageMetricReport;
  readonly generated_at: number | null;
  readonly stale: boolean;
  readonly refresh: {
    readonly state: 'idle' | 'pending' | 'deferred';
    readonly report_id: string | null;
    readonly retry_after_seconds: number;
  };
}

export type LatestUsageMetricQuery = Omit<ReturnType<typeof metricQuery>, 'from' | 'to'> &
  Partial<Pick<ReturnType<typeof metricQuery>, 'from' | 'to'>> & {
    readonly window: Selection['range'];
    readonly refresh: boolean;
  };

export type LatestMetricsClient = MetricsClient & {
  readonly latest?: (
    input: LatestUsageMetricQuery,
    options?: { readonly signal?: AbortSignal },
  ) => Promise<LatestUsageMetricView>;
};

export function latestQuery(
  selection: Selection,
  admin: boolean,
  refresh: boolean,
): LatestUsageMetricQuery {
  const query = metricQuery(selection, admin);
  if (selection.range !== 'custom') {
    const preset = { ...query };
    delete preset.from;
    delete preset.to;
    // The server aligns rolling endpoints independently. Its stable preset
    // resolution also accounts for partial boundary buckets (7d needs 600s).
    delete preset.step_seconds;
    return { ...preset, window: selection.range, refresh };
  }
  return { ...query, window: selection.range, refresh };
}

export function unsupportedLatestRoute(error: unknown): boolean {
  if (!(error instanceof ApiError)) return false;
  if (error.status === 501 && error.code === 'report_view_disabled') return true;
  // An old replica can answer an untyped router 404. A typed selected-resource
  // 404 has a code or extension and must remain an actual refusal.
  return (
    error.status === 404 && error.code === undefined && Object.keys(error.extensions).length === 0
  );
}
