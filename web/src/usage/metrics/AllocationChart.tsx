import './AllocationChart.css';
import { MeasuredChart } from './MeasuredChart';
import { useId, useMemo, useState } from 'react';
import type { UsageMetricPage, UsageMetricRow } from '@cma/client';
import { Bar, BarChart, CartesianGrid, Tooltip, XAxis, YAxis } from 'recharts';
import { OwnerAllocationChart } from './OwnerAllocationChart';
import type { MetricsFailure } from './failure';
import type { MetricsClient } from './model';
import { AllocationLabel } from './AllocationLabel';
import { Picker } from '../../ui/Picker';
import { Table, TableCell } from '../../ui/Table';
import { displayQuantity, type Metric } from './model';
import { sandboxCount } from './sandboxCount';
import { combineAllocationCoverage, type AllocationCoverageEvidence } from './allocationCoverage';
import { AllocationCoverageNote, AllocationCoverageStatus } from './AllocationCoverageStatus';

const resources = {
  cpu: { label: 'CPU', unit: 'core-hours', field: 'cpu_core_hours' },
  memory: { label: 'Memory', unit: 'GiB-hours', field: 'memory_gib_hours' },
  disk: { label: 'Disk', unit: 'GiB-hours', field: 'disk_gib_hours' },
} as const;

export function allocationComparisons(rows: readonly UsageMetricRow[], metric: Metric) {
  const groups = new Map<
    string,
    {
      id: string;
      name: string;
      value: number;
      history: ReturnType<typeof combineAllocationCoverage>;
    }
  >();
  for (const row of rows) {
    const id = row.sandbox_id;
    const existing = groups.get(id);
    groups.set(id, {
      id,
      name: row.name || 'Unnamed Sandbox',
      value: (existing?.value ?? 0) + Number(row.allocated_period[resources[metric].field]),
      history: combineAllocationCoverage(
        existing ? [existing.history, row.allocated_period] : [row.allocated_period],
      ),
    });
  }
  return [...groups.values()].sort((a, b) => b.value - a.value || a.id.localeCompare(b.id));
}

export function AllocationChart({
  page,
  admin,
  api,
  reportId,
  reportCoverage,
  active,
  metric,
  onMetricChange,
  onFailure,
}: {
  readonly page?: UsageMetricPage;
  readonly admin: boolean;
  readonly api: MetricsClient;
  readonly reportId: string;
  readonly reportCoverage?: AllocationCoverageEvidence;
  readonly active: boolean;
  readonly metric: Metric;
  readonly onMetricChange: (metric: Metric) => void;
  readonly onFailure: (failure: MetricsFailure) => void;
}) {
  const [group, setGroup] = useState<'sandbox' | 'owner'>('sandbox');
  const id = useId();
  const [tableOpen, setTableOpen] = useState(false);
  const resource = resources[metric];
  const rows = useMemo(
    () => allocationComparisons(page?.items ?? [], metric),
    [page?.items, metric],
  );
  const plotted = rows.slice(0, 8);
  const byOwner = admin && group === 'owner';
  if (!rows.length && !admin) return null;
  return (
    <section className="usage-allocation-chart" aria-label="Allocation comparison chart">
      <div className="usage-section-heading">
        <div>
          <h3>Allocation by {group === 'owner' && admin ? 'owner' : 'Sandbox'}</h3>
          <p id={id} hidden={byOwner || !page}>
            {page?.items.length ?? 0} of {sandboxCount(page?.total_count)} on this page.
            {rows.length > 8 ? ' Largest 8 shown; all page values are in the data table.' : ''}
          </p>
        </div>
        <div className="usage-metric-actions">
          <Picker
            label="Allocation chart resource"
            value={metric}
            options={Object.entries(resources).map(([value, item]) => ({
              value: value as Metric,
              label: item.label,
            }))}
            onChange={onMetricChange}
          />
          {admin && (
            <Picker
              label="Allocation chart grouping"
              value={group}
              options={[
                { value: 'sandbox', label: 'By Sandbox' },
                { value: 'owner', label: 'By owner' },
              ]}
              onChange={setGroup}
            />
          )}
        </div>
      </div>
      <p className="usage-chart-unit">
        {resource.label} · {resource.unit} in the report period
      </p>
      {reportCoverage && (
        <AllocationCoverageNote scope="Report allocation" value={reportCoverage} />
      )}
      {byOwner ? (
        <OwnerAllocationChart
          key={reportId}
          api={api}
          reportId={reportId}
          metric={metric}
          active={active}
          onFailure={onFailure}
        />
      ) : !rows.length ? (
        <p>{page ? 'No allocations on this page.' : 'Loading allocation chart…'}</p>
      ) : (
        <>
          <div
            className="usage-allocation-chart__plot"
            role="img"
            aria-label={`${resource.label} allocated ${resource.unit} by ${group}`}
            aria-describedby={id}
            style={{ height: Math.max(240, plotted.length * 40 + 40) }}
          >
            <MeasuredChart className="usage-allocation-chart__canvas">
              <BarChart
                data={plotted}
                layout="vertical"
                margin={{ top: 8, right: 24, bottom: 8, left: 0 }}
                accessibilityLayer={false}
              >
                <CartesianGrid
                  horizontal={false}
                  stroke="var(--dsw-alias-border-l2)"
                  strokeDasharray="3 3"
                />
                <XAxis
                  type="number"
                  tick={{ fill: 'var(--dsw-alias-label-secondary)', fontSize: 12 }}
                  tickLine={false}
                  axisLine={false}
                  tickFormatter={(value: number) =>
                    Intl.NumberFormat('en-SG', { notation: 'compact' }).format(value)
                  }
                />
                <YAxis
                  dataKey="id"
                  type="category"
                  width={136}
                  tickLine={false}
                  axisLine={false}
                  interval={0}
                  tick={({ x, y, payload }) => (
                    <AllocationLabel
                      x={Number(x)}
                      y={Number(y)}
                      name={
                        plotted.find((row) => row.id === payload.value)?.name ??
                        String(payload.value)
                      }
                    />
                  )}
                />
                <Tooltip
                  cursor={{ fill: 'var(--dsw-alias-interactive-bg-hover)' }}
                  labelFormatter={(value) => plotted.find((row) => row.id === value)?.name ?? value}
                  formatter={(value) => [displayQuantity(Number(value)), resource.unit]}
                  contentStyle={{
                    background: 'var(--dsw-alias-bg-layer-2)',
                    color: 'var(--dsw-alias-label-primary)',
                    borderColor: 'var(--dsw-alias-border-l2)',
                    borderRadius: 8,
                  }}
                />
                <Bar
                  dataKey="value"
                  name={resource.label}
                  fill="var(--dsw-alias-brand-primary)"
                  barSize={16}
                  radius={[0, 4, 4, 0]}
                  isAnimationActive={false}
                />
              </BarChart>
            </MeasuredChart>
          </div>
          <AllocationCoverageNote
            scope="Plotted Sandboxes"
            value={combineAllocationCoverage(plotted.map((row) => row.history))}
          />
          <details
            className="usage-chart-data"
            onToggle={(event) => setTableOpen(event.currentTarget.open)}
          >
            <summary>View allocation chart data</summary>
            {tableOpen && (
              <Table
                label="Allocation chart data"
                placement="usage-chart-table"
                columns={[
                  { header: 'Sandbox' },
                  { header: `${resource.label} (${resource.unit})`, align: 'end' },
                  { header: 'Coverage' },
                ]}
              >
                {rows.map((row) => (
                  <tr key={row.id}>
                    <TableCell>
                      <span title={row.id}>{row.name}</span>
                    </TableCell>
                    <TableCell align="end">{displayQuantity(row.value)}</TableCell>
                    <TableCell>
                      <AllocationCoverageStatus value={row.history} />
                    </TableCell>
                  </tr>
                ))}
              </Table>
            )}
          </details>
        </>
      )}
    </section>
  );
}
