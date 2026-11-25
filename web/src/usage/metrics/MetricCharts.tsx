import { useCallback, useEffect } from 'react';
import type { UsageMetricReport, UsageMetricSeries } from '@cma/client';
import { MetricChart } from './MetricChart';
import { MetricsNotice } from './MetricsNotice';
import { useUsageRead } from './useReport';
import type { Group, Metric, MetricsClient } from './model';
import type { MetricsFailure } from './failure';

export function MetricCharts({
  api,
  report,
  group,
  metric,
  active,
  initialCharts,
  onFailure,
}: {
  readonly api: MetricsClient;
  readonly report: UsageMetricReport;
  readonly group: Group;
  readonly metric?: Metric;
  readonly active: boolean;
  readonly initialCharts?: readonly UsageMetricSeries[];
  readonly onFailure: (failure: MetricsFailure) => void;
}) {
  const load = useCallback(
    (signal: AbortSignal) =>
      Promise.all(
        (metric ? [metric] : (['cpu', 'memory', 'disk'] as const)).map((value) =>
          api.series(report.report_id, { signal, metric: value, group_by: group, top: 10 }),
        ),
      ),
    [api, report.report_id, group, metric],
  );
  const requestedMetrics: readonly Metric[] = metric ? [metric] : ['cpu', 'memory', 'disk'];
  const initial =
    group === 'total' &&
    initialCharts &&
    requestedMetrics.every((requested) =>
      initialCharts.some((series) => series.metric === requested),
    )
      ? initialCharts.filter((series) => requestedMetrics.includes(series.metric))
      : undefined;
  const charts = useUsageRead(load, active && initial === undefined, api);
  useEffect(() => {
    if (charts.error?.denied || charts.error?.expired) onFailure(charts.error);
  }, [charts.error, onFailure]);
  return (
    <section aria-label="Measured resource charts">
      <MetricsNotice failure={charts.error} onRetry={charts.retry} />
      {charts.loading && <p role="status">Loading measured charts…</p>}
      {group === 'owner' && <p>Top owners and Other cover the entire selected cohort.</p>}
      {(initial ?? charts.data)?.map((series) => (
        <MetricChart
          key={series.metric}
          series={series}
          capacity={
            group === 'total'
              ? report.summary?.total_allocated_capacity[
                  series.metric === 'cpu'
                    ? 'cpu_cores'
                    : series.metric === 'memory'
                      ? 'memory_bytes'
                      : 'disk_bytes'
                ]
              : undefined
          }
        />
      ))}
    </section>
  );
}
