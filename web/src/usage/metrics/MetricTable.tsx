import { useCallback, useEffect, useState, type ReactNode } from 'react';
import type { UsageMetricRow, UsageMetricSort } from '@chronoai/cma-sdk';
import { Button } from '../../ui/Button';
import { StatusBadge } from '../../ui/Badge';
import { Table, TableActions, TableCell, type TableColumn } from '../../ui/Table';
import { useUsageRead } from './useReport';
import { displayQuantity, metricTime, metricValue, type Metric, type MetricsClient } from './model';
import { MetricsNotice } from './MetricsNotice';
import type { MetricsFailure } from './failure';

export function MetricTable({
  api,
  reportId,
  sort,
  order,
  admin,
  active,
  measured,
  focusedMetric,
  onSort,
  onDetail,
  onFailure,
  actions,
}: {
  readonly api: MetricsClient;
  readonly reportId: string;
  readonly sort: UsageMetricSort;
  readonly order: 'asc' | 'desc';
  readonly admin: boolean;
  readonly active: boolean;
  readonly measured: boolean;
  readonly focusedMetric?: Metric;
  readonly onSort: (sort: UsageMetricSort) => void;
  readonly onDetail: (row: UsageMetricRow) => void;
  readonly onFailure: (failure: MetricsFailure) => void;
  readonly actions?: (row: UsageMetricRow) => ReactNode;
}) {
  const [cursors, setCursors] = useState<readonly (string | undefined)[]>([undefined]);
  const cursor = cursors[cursors.length - 1];
  const load = useCallback(
    (signal: AbortSignal) => api.sandboxes(reportId, { signal, limit: 50, cursor, sort, order }),
    [api, reportId, cursor, sort, order],
  );
  const records = useUsageRead(load, active);
  useEffect(() => {
    if (records.error?.denied || records.error?.expired) onFailure(records.error);
  }, [records.error, onFailure]);
  const column = (header: string, key?: UsageMetricSort): TableColumn => ({
    header,
    ...(key
      ? {
          sort:
            sort === key
              ? order === 'asc'
                ? ('ascending' as const)
                : ('descending' as const)
              : undefined,
          onSort: () => onSort(key),
        }
      : {}),
  });
  const metrics: readonly Metric[] = focusedMetric ? [focusedMetric] : ['cpu', 'memory', 'disk'];
  const columns = [
    column('Sandbox', 'name'),
    ...(admin ? [column('Owner', 'owner')] : []),
    column('Status', 'status'),
    column('Runtime'),
    column('Agents'),
    ...metrics.map((metric) =>
      measured
        ? column(
            metric === 'cpu'
              ? 'CPU mean / allocated (cores)'
              : metric === 'memory'
                ? 'Memory mean / allocated (GiB)'
                : 'Disk used / allocated (GiB)',
            metric === 'cpu'
              ? 'cpu_mean_cores'
              : metric === 'memory'
                ? 'memory_mean_bytes'
                : 'disk_latest_bytes',
          )
        : column(
            metric === 'cpu'
              ? 'CPU allocated (cores)'
              : metric === 'memory'
                ? 'Memory allocated (GiB)'
                : 'Disk allocated (GiB)',
          ),
    ),
    column('Allocated CPU (core-hours)', 'allocated_cpu_core_hours'),
    column('Allocated memory (GiB-hours)', 'allocated_memory_gib_hours'),
    column('Allocated disk (GiB-hours)', 'allocated_disk_gib_hours'),
    column('Last activity (SGT)', 'last_activity_at'),
  ];
  return (
    <section aria-label="Usage inventory">
      <MetricsNotice failure={records.error} onRetry={records.retry} />
      <Table
        label={measured ? 'Sandboxes and measured usage' : 'Sandboxes and allocated usage'}
        columns={columns}
        actions={actions ? 'Actions' : undefined}
        loading={records.loading}
        loadingLabel="Loading usage page"
        empty={records.data ? 'No Sandboxes match this selection.' : undefined}
      >
        {records.data?.items.map((row) => (
          <tr key={row.sandbox_id}>
            <TableCell>
              <Button size="sm" onClick={() => onDetail(row)}>
                {row.name || row.sandbox_id}
              </Button>
              <code className="usage-metric-id">{row.sandbox_id}</code>
            </TableCell>
            {admin && <TableCell>{row.owner.display_name || row.owner.subject}</TableCell>}
            <TableCell>
              <StatusBadge status={row.status} />
            </TableCell>
            <TableCell>{row.runtime_version ?? 'Not bound'}</TableCell>
            <TableCell>{displayQuantity(row.agent_count)}</TableCell>
            {metrics.map((metric) => (
              <TableCell key={metric}>
                {measured ? (
                  <CompactReading metric={metric} row={row} />
                ) : (
                  metricValue(
                    row.allocation[
                      metric === 'cpu'
                        ? 'cpu_cores'
                        : metric === 'memory'
                          ? 'memory_bytes'
                          : 'disk_bytes'
                    ],
                    metric,
                  )
                )}
              </TableCell>
            ))}
            <TableCell>{displayQuantity(row.allocated_period.cpu_core_hours)}</TableCell>
            <TableCell>{displayQuantity(row.allocated_period.memory_gib_hours)}</TableCell>
            <TableCell>
              {displayQuantity(row.allocated_period.disk_gib_hours)}
              <span className="usage-metric-id">
                Allocation history: {row.allocated_period.complete ? 'complete' : 'partial'}
              </span>
            </TableCell>
            <TableCell>{metricTime(row.last_activity_at)}</TableCell>
            {actions && <TableActions>{actions(row)}</TableActions>}
          </tr>
        ))}
      </Table>
      <div className="usage-metric-actions">
        <span>
          {records.data ? `${displayQuantity(records.data.total_count)} Sandboxes · ` : ''}50 per
          page · page {cursors.length}
        </span>
        <Button
          size="sm"
          disabled={records.loading || cursors.length === 1}
          onClick={() => setCursors((previous) => previous.slice(0, -1))}
        >
          Previous usage page
        </Button>
        <Button
          size="sm"
          disabled={records.loading || !records.data?.next}
          onClick={() => {
            const next = records.data?.next;
            if (next && !cursors.includes(next)) setCursors((previous) => [...previous, next]);
          }}
        >
          Next usage page
        </Button>
      </div>
    </section>
  );
}

function CompactReading({
  row,
  metric,
}: {
  readonly row: UsageMetricRow;
  readonly metric: Metric;
}) {
  const reading = row.measured[metric];
  const value =
    metric === 'cpu'
      ? row.measured.cpu.mean_cores
      : metric === 'memory'
        ? row.measured.memory.mean_bytes
        : row.measured.disk.latest_bytes;
  const capacity =
    row.allocation[
      metric === 'cpu' ? 'cpu_cores' : metric === 'memory' ? 'memory_bytes' : 'disk_bytes'
    ];
  return (
    <>
      <span>
        {metricValue(value, metric)} / {metricValue(capacity, metric)}
      </span>
      <span className="usage-metric-id">
        {reading.state.replaceAll('_', ' ')} · coverage{' '}
        {reading.coverage_ratio == null
          ? 'unavailable'
          : `${displayQuantity(Number(reading.coverage_ratio) * 100)}%`}
      </span>
      <span className="usage-metric-id">Observed {metricTime(reading.observed_at)}</span>
      {value === null && reading.last_observed !== null && (
        <span className="usage-metric-id">
          Last observed {metricValue(reading.last_observed, metric)}
        </span>
      )}
    </>
  );
}
