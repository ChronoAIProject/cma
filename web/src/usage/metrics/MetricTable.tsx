import { useCallback, useEffect, useState, type ReactNode } from 'react';
import type { UsageMetricPage, UsageMetricRow, UsageMetricSort } from '@cma/client';
import { Button } from '../../ui/Button';
import { Picker } from '../../ui/Picker';
import { AllocationChart } from './AllocationChart';
import { UsageInventoryRow } from './UsageInventoryRow';
import { Table, type TableColumn } from '../../ui/Table';
import { useUsageRead } from './useReport';
import type { Metric, MetricsClient } from './model';
import { sandboxCount } from './sandboxCount';
import { MetricsNotice } from './MetricsNotice';
import './UsageInventory.css';
import type { MetricsFailure } from './failure';
import type { AllocationCoverageEvidence } from './allocationCoverage';

export function MetricTable({
  api,
  reportId,
  reportCoverage,
  sort,
  order,
  admin,
  active,
  measured,
  allocationMetric,
  onAllocationMetricChange,
  initialPage,
  onSort,
  onDetail,
  onFailure,
  actions,
}: {
  readonly api: MetricsClient;
  readonly reportId: string;
  readonly reportCoverage?: AllocationCoverageEvidence;
  readonly sort: UsageMetricSort;
  readonly order: 'asc' | 'desc';
  readonly admin: boolean;
  readonly active: boolean;
  readonly measured: boolean;
  readonly allocationMetric: Metric;
  readonly onAllocationMetricChange: (metric: Metric) => void;
  readonly initialPage?: UsageMetricPage;
  readonly focusedMetric?: Metric;
  readonly onSort: (sort: UsageMetricSort) => void;
  readonly onDetail: (row: UsageMetricRow) => void;
  readonly onFailure: (failure: MetricsFailure) => void;
  readonly actions?: (row: UsageMetricRow) => ReactNode;
}) {
  const pageKey = `${sort}:${order}`;
  const [paging, setPaging] = useState<{ key: string; cursors: readonly (string | undefined)[] }>({
    key: pageKey,
    cursors: [undefined],
  });
  if (paging.key !== pageKey) setPaging({ key: pageKey, cursors: [undefined] });
  const cursors = paging.key === pageKey ? paging.cursors : [undefined];
  const setCursors = (
    update: (previous: readonly (string | undefined)[]) => readonly (string | undefined)[],
  ) => setPaging({ key: pageKey, cursors: update(cursors) });
  const cursor = cursors[cursors.length - 1];
  const load = useCallback(
    (signal: AbortSignal) => api.sandboxes(reportId, { signal, limit: 50, cursor, sort, order }),
    [api, reportId, cursor, sort, order],
  );
  const records = useUsageRead(load, active && !(cursor === undefined && initialPage), api);
  const page = cursor === undefined && initialPage ? initialPage : records.data;
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
  const columns = [
    column('Sandbox', 'name'),
    column('CPU (core-hours)', 'allocated_cpu_core_hours'),
    column('Memory (GiB-hours)', 'allocated_memory_gib_hours'),
    column('Disk (GiB-hours)', 'allocated_disk_gib_hours'),
  ];
  const sortOptions: { value: UsageMetricSort; label: string }[] = [
    { value: 'name', label: 'Sandbox name' },
    ...(admin ? [{ value: 'owner' as const, label: 'Owner' }] : []),
    { value: 'status', label: 'Status' },
    { value: 'last_activity_at', label: 'Last activity' },
    { value: 'allocated_cpu_core_hours', label: 'Allocated CPU' },
    { value: 'allocated_memory_gib_hours', label: 'Allocated memory' },
    { value: 'allocated_disk_gib_hours', label: 'Allocated disk' },
    ...(measured
      ? [
          { value: 'cpu_mean_cores' as const, label: 'Measured CPU' },
          { value: 'memory_mean_bytes' as const, label: 'Measured memory' },
          { value: 'disk_latest_bytes' as const, label: 'Measured disk' },
        ]
      : []),
  ];
  return (
    <section className="usage-inventory" aria-label="Usage inventory">
      <AllocationChart
        page={page}
        admin={admin}
        api={api}
        reportId={reportId}
        reportCoverage={reportCoverage}
        active={active}
        metric={allocationMetric}
        onMetricChange={onAllocationMetricChange}
        onFailure={onFailure}
      />
      <div className="usage-section-heading">
        <div>
          <h3>Sandboxes</h3>
          <p>Allocated resource-hours for the report period</p>
        </div>
        <div className="usage-metric-actions">
          <Picker
            label="Sort usage inventory"
            value={sort}
            options={sortOptions}
            onChange={onSort}
          />
          <Button
            size="sm"
            variant="outline"
            aria-label={`Sort ${order === 'asc' ? 'descending' : 'ascending'}`}
            onClick={() => onSort(sort)}
          >
            {order === 'asc' ? 'Ascending ↑' : 'Descending ↓'}
          </Button>
        </div>
      </div>
      <MetricsNotice failure={records.error} onRetry={records.retry} />
      <Table
        label={measured ? 'Sandboxes and measured usage' : 'Sandboxes and allocated usage'}
        columns={columns}
        placement="usage-inventory-table"
        loading={records.loading && !page}
        loadingLabel="Loading usage page"
        empty={page ? 'No Sandboxes match this selection.' : undefined}
      >
        {page?.items.map((row) => (
          <UsageInventoryRow
            key={row.sandbox_id}
            row={row}
            admin={admin}
            measured={measured}
            onDetail={onDetail}
            actions={actions}
          />
        ))}
      </Table>
      <div className="usage-metric-actions">
        <span>
          {page ? `${sandboxCount(page.total_count)} · ` : ''}50 per page · page {cursors.length}
        </span>
        <Button
          size="sm"
          variant="outline"
          disabled={records.loading || cursors.length === 1}
          onClick={() => setCursors((previous) => previous.slice(0, -1))}
        >
          Previous usage page
        </Button>
        <Button
          size="sm"
          variant="outline"
          disabled={records.loading || !page?.next}
          onClick={() => {
            const next = page?.next;
            if (next && !cursors.includes(next)) setCursors((previous) => [...previous, next]);
          }}
        >
          Next usage page
        </Button>
      </div>
    </section>
  );
}
