import { MeasuredChart } from './MeasuredChart';
import { useMemo, useState } from 'react';
import { CartesianGrid, Line, LineChart, ReferenceLine, Tooltip, XAxis, YAxis } from 'recharts';
import type { UsageMetricQuantity, UsageMetricSeries } from '@cma/client';
import { Button } from '../../ui/Button';
import { Picker } from '../../ui/Picker';
import { Table, TableCell } from '../../ui/Table';
import { displayQuantity, metricLabels, metricTime, metricValue } from './model';

const strokes = [
  'var(--dsw-alias-brand-primary)',
  'var(--dsw-alias-label-primary)',
  'var(--dsw-alias-label-secondary)',
];
const dashes = ['', '8 4', '2 4', '8 3 2 3'];

export function MetricChart({
  series,
  capacity,
}: {
  readonly series: UsageMetricSeries;
  readonly capacity?: UsageMetricQuantity | null;
}) {
  const [tableOpen, setTableOpen] = useState(false);
  const divisor = series.metric === 'cpu' ? 1 : 2 ** 30;
  const points = useMemo(() => {
    const rows = new Map<number, Record<string, number | null>>();
    series.lines.forEach((line, index) => {
      for (const point of line.points) {
        const at = Number(point.from);
        const row = rows.get(at) ?? { at };
        row[`value${index}`] = point.mean === null ? null : Number(point.mean) / divisor;
        rows.set(at, row);
      }
    });
    return [...rows.values()].sort((a, b) => Number(a.at) - Number(b.at));
  }, [series, divisor]);
  const hasData = series.lines.some((line) => line.points.some((point) => point.mean !== null));
  return (
    <section className="usage-metric-chart" aria-label={`${metricLabels[series.metric]} chart`}>
      <h3>
        {metricLabels[series.metric]} ({series.metric === 'cpu' ? 'cores' : 'GiB'})
      </h3>
      <p>Source: {series.source}. Gaps indicate unavailable measurements.</p>
      {hasData ? (
        <MeasuredChart className="usage-metric-chart__plot">
          <LineChart
            accessibilityLayer={false}
            data={points}
            margin={{ top: 12, right: 16, bottom: 8, left: 0 }}
          >
            <CartesianGrid stroke="var(--dsw-alias-border-l1)" strokeDasharray="3 3" />
            <XAxis
              dataKey="at"
              type="number"
              domain={['dataMin', 'dataMax']}
              tickFormatter={(value: number) =>
                new Date(value * 1000).toLocaleString('en-SG', {
                  timeZone: 'Asia/Singapore',
                  month: 'short',
                  day: 'numeric',
                  hour: '2-digit',
                  minute: '2-digit',
                })
              }
              tick={{ fill: 'var(--dsw-alias-label-secondary)', fontSize: 12 }}
            />
            <YAxis
              domain={[0, 'auto']}
              tick={{ fill: 'var(--dsw-alias-label-secondary)', fontSize: 12 }}
              width={60}
            />
            <Tooltip
              contentStyle={{
                background: 'var(--dsw-alias-bg-layer-2)',
                color: 'var(--dsw-alias-label-primary)',
                borderColor: 'var(--dsw-alias-border-l1)',
              }}
              labelFormatter={(value) => metricTime(Number(value))}
            />
            {capacity != null && (
              <ReferenceLine
                y={Number(capacity) / divisor}
                stroke="var(--dsw-alias-label-secondary)"
                strokeDasharray="5 5"
                ifOverflow="extendDomain"
              />
            )}
            {series.lines.map((line, index) => (
              <Line
                key={line.id}
                dataKey={`value${index}`}
                name={line.label}
                type="linear"
                connectNulls={false}
                stroke={strokes[index % strokes.length]}
                strokeDasharray={dashes[Math.floor(index / strokes.length)]}
                strokeWidth={2}
                dot={false}
                isAnimationActive={false}
              />
            ))}
          </LineChart>
        </MeasuredChart>
      ) : (
        <p role="status">
          {series.lines.length
            ? 'No measured samples in this range.'
            : 'No Sandboxes in this selection.'}
        </p>
      )}
      {capacity != null && (
        <p>
          Dashed reference: allocated now {metricValue(capacity, series.metric)} (report snapshot).
        </p>
      )}
      <ul className="usage-metric-chart__legend" aria-label="Chart series">
        {series.lines.map((line, index) => (
          <li key={line.id}>
            <svg width="24" height="8" aria-hidden="true">
              <line
                x1="0"
                x2="24"
                y1="4"
                y2="4"
                stroke={strokes[index % strokes.length]}
                strokeDasharray={dashes[Math.floor(index / strokes.length)]}
                strokeWidth="2"
              />
            </svg>
            {line.label}
          </li>
        ))}
      </ul>
      <details onToggle={(event) => setTableOpen(event.currentTarget.open)}>
        <summary>View {metricLabels[series.metric]} chart data</summary>
        {tableOpen && (
          <ChartTable
            key={series.report_id + series.metric + series.lines.map((line) => line.id).join(':')}
            series={series}
          />
        )}
      </details>
    </section>
  );
}

function ChartTable({ series }: { readonly series: UsageMetricSeries }) {
  const [lineId, setLineId] = useState(series.lines[0]?.id ?? '');
  const [page, setPage] = useState(0);
  const line = series.lines.find((entry) => entry.id === lineId);
  const points = line?.points ?? [];
  return (
    <>
      <Picker
        label="Chart data series"
        value={lineId}
        options={series.lines.map((entry) => ({ value: entry.id, label: entry.label }))}
        onChange={(value) => {
          setLineId(value);
          setPage(0);
        }}
      />
      <Table
        label={`${metricLabels[series.metric]} chart data`}
        columns={[
          { header: 'Interval (SGT)' },
          { header: 'Mean' },
          { header: 'Minimum / maximum' },
          { header: 'Covered / expected seconds' },
          { header: 'Matching capacity used' },
          { header: 'Coverage state' },
        ]}
        empty="No chart buckets for this selection."
      >
        {points.slice(page * 50, (page + 1) * 50).map((point) => (
          <tr key={String(point.from)}>
            <TableCell>
              {metricTime(point.from)} – {metricTime(point.to)}
            </TableCell>
            <TableCell>{metricValue(point.mean, series.metric)}</TableCell>
            <TableCell>
              {metricValue(point.min, series.metric)} / {metricValue(point.max, series.metric)}
            </TableCell>
            <TableCell>
              {displayQuantity(point.covered_seconds)} / {displayQuantity(point.expected_seconds)};
              unknown mapping {displayQuantity(point.unknown_mapping_seconds)}
            </TableCell>
            <TableCell>
              {point.numerator !== null &&
              point.denominator !== null &&
              Number(point.denominator) > 0
                ? `${displayQuantity((Number(point.numerator) / Number(point.denominator)) * 100)}%`
                : 'Unavailable'}
            </TableCell>
            <TableCell>
              {point.reason?.replaceAll('_', ' ') ?? 'Observed'};{' '}
              {displayQuantity(point.excluded_count)} excluded
            </TableCell>
          </tr>
        ))}
      </Table>
      <div className="usage-metric-actions">
        <Button size="sm" disabled={page === 0} onClick={() => setPage((value) => value - 1)}>
          Previous chart data
        </Button>
        <span>Chart data page {page + 1}</span>
        <Button
          size="sm"
          disabled={(page + 1) * 50 >= points.length}
          onClick={() => setPage((value) => value + 1)}
        >
          Next chart data
        </Button>
      </div>
    </>
  );
}
