import './AllocationChart.css';
import { useId, useMemo, useState } from 'react';
import type { UsageMetricPage, UsageMetricRow } from '@chronoai/cma-sdk';
import { Bar, BarChart, CartesianGrid, ResponsiveContainer, Tooltip, XAxis, YAxis } from 'recharts';
import { AllocationLabel } from './AllocationLabel';
import { Picker } from '../../ui/Picker';
import { Table, TableCell } from '../../ui/Table';
import { displayQuantity, type Metric } from './model';

const resources = {
  cpu: { label: 'CPU', unit: 'core-hours', field: 'cpu_core_hours' },
  memory: { label: 'Memory', unit: 'GiB-hours', field: 'memory_gib_hours' },
  disk: { label: 'Disk', unit: 'GiB-hours', field: 'disk_gib_hours' },
} as const;

export function allocationComparisons(
  rows: readonly UsageMetricRow[],
  metric: Metric,
  owner: boolean,
) {
  const groups = new Map<string, { id: string; name: string; value: number; complete: boolean }>();
  for (const row of rows) {
    const id = owner ? row.owner.subject : row.sandbox_id;
    const existing = groups.get(id);
    groups.set(id, {
      id,
      name: owner ? row.owner.display_name || row.owner.subject : row.name || 'Unnamed Sandbox',
      value: (existing?.value ?? 0) + Number(row.allocated_period[resources[metric].field]),
      complete: (existing?.complete ?? true) && row.allocated_period.complete,
    });
  }
  return [...groups.values()].sort((a, b) => b.value - a.value || a.id.localeCompare(b.id));
}

export function AllocationChart({
  page,
  admin,
}: {
  readonly page: UsageMetricPage;
  readonly admin: boolean;
}) {
  const [metric, setMetric] = useState<Metric>('cpu');
  const [group, setGroup] = useState<'sandbox' | 'owner'>('sandbox');
  const id = useId();
  const [tableOpen, setTableOpen] = useState(false);
  const resource = resources[metric];
  const rows = useMemo(
    () => allocationComparisons(page.items, metric, admin && group === 'owner'),
    [page.items, metric, admin, group],
  );
  const plotted = rows.slice(0, 8);
  if (!rows.length) return null;
  return (
    <section className="usage-allocation-chart" aria-label="Allocation comparison chart">
      <div className="usage-section-heading">
        <div>
          <h3>Allocation by {group === 'owner' && admin ? 'owner' : 'Sandbox'}</h3>
          <p id={id}>
            {page.items.length} of {displayQuantity(page.total_count)} Sandboxes on this page.
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
            onChange={setMetric}
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
        {resource.label} · {resource.unit} in the selected period
      </p>
      <div
        className="usage-allocation-chart__plot"
        role="img"
        aria-label={`${resource.label} allocated ${resource.unit} by ${group}`}
        aria-describedby={id}
        style={{ height: Math.max(240, plotted.length * 40 + 40) }}
      >
        <div aria-hidden="true" className="usage-allocation-chart__canvas">
          <ResponsiveContainer width="100%" height="100%" minWidth={0}>
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
                      plotted.find((row) => row.id === payload.value)?.name ?? String(payload.value)
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
          </ResponsiveContainer>
        </div>
      </div>
      {rows.some((row) => !row.complete) && (
        <p className="usage-metric-coverage">
          Partial history: bars show recorded allocation only, including any extension from the last
          known state. Missing history is excluded.
        </p>
      )}
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
              { header: group === 'owner' ? 'Owner' : 'Sandbox' },
              { header: `${resource.label} (${resource.unit})`, align: 'end' },
              { header: 'History' },
            ]}
          >
            {rows.map((row) => (
              <tr key={row.id}>
                <TableCell>
                  <span title={row.id}>{row.name}</span>
                </TableCell>
                <TableCell align="end">{displayQuantity(row.value)}</TableCell>
                <TableCell>{row.complete ? 'Complete' : 'Partial'}</TableCell>
              </tr>
            ))}
          </Table>
        )}
      </details>
    </section>
  );
}
