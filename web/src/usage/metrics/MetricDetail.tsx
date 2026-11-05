import { useCallback, useEffect, useState, type ReactNode } from 'react';
import type { Cma, UsageMetricRow } from '@chronoai/cma-sdk';
import { Button } from '../../ui/Button';
import { UsageHistory } from '../UsageHistory';
import { MetricChart } from './MetricChart';
import { AllocatedPeriod, MetricReading } from './MetricSummary';
import { MetricsNotice } from './MetricsNotice';
import { useUsageRead } from './useReport';
import { metricLabels, type MetricsClient } from './model';
import type { MetricsFailure } from './failure';

export function MetricDetail({
  api,
  history,
  reportId,
  sandboxId,
  active,
  admin,
  onClose,
  onOwner,
  onFailure,
  actions,
}: {
  readonly api: MetricsClient;
  readonly history: Pick<Cma['usage'], 'events'>;
  readonly reportId: string;
  readonly sandboxId: string;
  readonly active: boolean;
  readonly admin: boolean;
  readonly onClose: () => void;
  readonly onOwner?: (owner: string) => void;
  readonly onFailure: (failure: MetricsFailure) => void;
  readonly actions?: (row: UsageMetricRow) => ReactNode;
}) {
  const [showHistory, setShowHistory] = useState(false);
  const load = useCallback(
    async (signal: AbortSignal) => {
      const [row, ...series] = await Promise.all([
        api.sandbox(reportId, sandboxId, { signal }),
        ...(['cpu', 'memory', 'disk'] as const).map((metric) =>
          api.series(reportId, { metric, group_by: 'sandbox', series_id: sandboxId, signal }),
        ),
      ]);
      return { row, series };
    },
    [api, reportId, sandboxId],
  );
  const detail = useUsageRead(load, active);
  useEffect(() => {
    if (detail.error?.denied || detail.error?.expired) onFailure(detail.error);
  }, [detail.error, onFailure]);
  return (
    <section className="usage-metric-detail" aria-label="Sandbox usage details">
      <h2>{detail.data?.row.name ?? sandboxId}</h2>
      <Button size="sm" onClick={onClose}>
        Close usage details
      </Button>
      <MetricsNotice failure={detail.error} onRetry={detail.retry} />
      {detail.loading && <p role="status">Loading Sandbox usage details…</p>}
      {detail.data && (
        <>
          <p>
            <code>{detail.data.row.sandbox_id}</code> ·{' '}
            {detail.data.row.status.replaceAll('_', ' ')} · Runtime{' '}
            {detail.data.row.runtime_version ?? 'not bound'}
          </p>
          {detail.data.row.shared_sandbox_usage && (
            <p>Shared Sandbox usage: multiple Agents share these CPU, memory and disk readings.</p>
          )}
          {admin && (
            <Button size="sm" onClick={() => onOwner?.(detail.data!.row.owner.subject)}>
              View this owner's usage
            </Button>
          )}
          {actions?.(detail.data.row)}
          <div className="usage-metric-cards">
            {(['cpu', 'memory', 'disk'] as const).map((metric) => (
              <section key={metric} aria-label={metricLabels[metric]}>
                <h3>{metricLabels[metric]}</h3>
                <MetricReading
                  metric={metric}
                  reading={detail.data!.row.measured[metric]}
                  capacity={
                    detail.data!.row.allocation[
                      metric === 'cpu'
                        ? 'cpu_cores'
                        : metric === 'memory'
                          ? 'memory_bytes'
                          : 'disk_bytes'
                    ]
                  }
                />
              </section>
            ))}
          </div>
          <AllocatedPeriod value={detail.data.row.allocated_period} />
          {detail.data.series.map((series) => (
            <MetricChart
              key={series.metric}
              series={series}
              capacity={
                detail.data!.row.allocation[
                  series.metric === 'cpu'
                    ? 'cpu_cores'
                    : series.metric === 'memory'
                      ? 'memory_bytes'
                      : 'disk_bytes'
                ]
              }
            />
          ))}
          <Button size="sm" onClick={() => setShowHistory((value) => !value)}>
            Lifecycle history
          </Button>
          {showHistory && active && (
            <UsageHistory client={history} id={sandboxId} onClose={() => setShowHistory(false)} />
          )}
        </>
      )}
    </section>
  );
}
