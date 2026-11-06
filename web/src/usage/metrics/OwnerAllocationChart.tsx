import './OwnerAllocationChart.css';
import { useEffect, useId, useRef, useState } from 'react';
import { Bar, BarChart, CartesianGrid, ResponsiveContainer, Tooltip, XAxis, YAxis } from 'recharts';
import { Picker } from '../../ui/Picker';
import { Button } from '../../ui/Button';
import { OwnerAllocationTable } from './OwnerAllocationTable';
import { MetricsNotice } from './MetricsNotice';
import { displayQuantity, type Metric, type MetricsClient } from './model';
import type { MetricsFailure } from './failure';
import { useOwnerAllocations } from './useOwnerAllocations';

const ROW = 48;
const HEIGHT = ROW * 8;
const BUFFER = 2;
type Count = '5' | '10' | '50' | 'all';
const choices = ['5', '10', '50', 'all'].map((value) => ({
  value: value as Count,
  label: value === 'all' ? 'All' : `Top ${value}`,
}));

export function OwnerAllocationChart({
  api,
  reportId,
  metric,
  active,
  onFailure,
}: {
  readonly api: MetricsClient;
  readonly reportId: string;
  readonly metric: Metric;
  readonly active: boolean;
  readonly onFailure: (failure: MetricsFailure) => void;
}) {
  const data = useOwnerAllocations(api, reportId, metric, active);
  const [count, setCount] = useState<Count>('10');
  const [position, setPosition] = useState({ metric, offset: 0 });
  const offset = position.metric === metric ? position.offset : 0;
  const scroll = useRef<HTMLDivElement>(null);
  const description = useId();
  useEffect(() => {
    if (scroll.current) scroll.current.scrollTop = 0;
  }, [metric]);
  const size = Math.min(data.items.length, count === 'all' ? Infinity : Number(count));
  const start = Math.max(0, Math.min(Math.floor(offset / ROW) - BUFFER, Math.max(0, size - 1)));
  const end = Math.min(size, start + HEIGHT / ROW + BUFFER * 2);
  const rows = data.items.slice(start, end).map((row, index) => ({
    ...row,
    position: start + index + 1,
    value: Number(row.allocated_hours),
  }));
  const max = Math.max(1, Number(data.items[0]?.allocated_hours ?? 0));
  const ticks = [0, max / 4, max / 2, (max * 3) / 4, max];
  const unit = metric === 'cpu' ? 'core-hours' : 'GiB-hours';
  const partial = data.items.slice(0, size).some((item) => !item.complete);
  useEffect(() => {
    if (data.error?.denied || data.error?.expired) onFailure(data.error);
  }, [data.error, onFailure]);
  const reset = () => {
    if (scroll.current) scroll.current.scrollTop = 0;
    setPosition({ metric, offset: 0 });
  };
  return (
    <div className="usage-owner-allocation">
      <div className="usage-metric-actions">
        <Picker
          label="Owners shown"
          value={count}
          options={choices}
          onChange={(value) => {
            setCount(value);
            reset();
          }}
        />
        <p id={description} aria-live="polite">
          {data.total === undefined
            ? data.error
              ? 'Owner allocations are unavailable.'
              : 'Loading owner allocations…'
            : data.total === 0
              ? 'No owners match this selection.'
              : count === 'all'
                ? `${data.items.length.toLocaleString()} of ${data.total.toLocaleString()} owners loaded.${data.next ? ' Scroll or load more to see the rest.' : ' All owners loaded.'}`
                : `${size} of ${data.total.toLocaleString()} owners · largest allocation first`}
        </p>
      </div>
      <MetricsNotice failure={data.error} onRetry={data.retry} />
      {size > 0 && (
        <div className="usage-owner-allocation__frame">
          <div
            className="usage-owner-allocation__scroll"
            ref={scroll}
            tabIndex={0}
            role="region"
            aria-label="Scrollable owner allocations"
            aria-describedby={description}
            style={{ height: Math.min(HEIGHT, size * ROW) }}
            onScroll={(event) => {
              const view = event.currentTarget;
              setPosition({ metric, offset: view.scrollTop });
              if (
                count === 'all' &&
                data.next &&
                !data.error &&
                view.scrollTop + view.clientHeight >= view.scrollHeight - ROW * 2
              )
                data.loadMore();
            }}
          >
            <div style={{ height: size * ROW, position: 'relative' }}>
              <div
                className="usage-owner-allocation__window"
                style={{ top: start * ROW, height: rows.length * ROW }}
              >
                <div aria-hidden="true" className="usage-owner-allocation__bars">
                  <ResponsiveContainer width="100%" height="100%" minWidth={0}>
                    <BarChart
                      data={rows}
                      layout="vertical"
                      margin={{ left: 136, right: 24, top: 0, bottom: 0 }}
                      accessibilityLayer={false}
                    >
                      <CartesianGrid
                        horizontal={false}
                        stroke="var(--dsw-alias-border-l2)"
                        strokeDasharray="3 3"
                      />
                      <XAxis type="number" domain={[0, max]} ticks={ticks} hide />
                      <YAxis type="category" dataKey="position" hide />
                      <Tooltip
                        labelFormatter={(position) =>
                          rows.find((row) => row.position === Number(position))?.label ?? 'Owner'
                        }
                        formatter={(value) => [displayQuantity(Number(value)), unit]}
                        cursor={{ fill: 'var(--dsw-alias-interactive-bg-hover)' }}
                        contentStyle={{
                          background: 'var(--dsw-alias-bg-layer-2)',
                          color: 'var(--dsw-alias-label-primary)',
                          borderColor: 'var(--dsw-alias-border-l2)',
                          borderRadius: 8,
                        }}
                      />
                      <Bar
                        dataKey="value"
                        fill="var(--dsw-alias-brand-primary)"
                        barSize={16}
                        radius={[0, 4, 4, 0]}
                        isAnimationActive={false}
                      />
                    </BarChart>
                  </ResponsiveContainer>
                </div>
                <ol
                  className="usage-owner-allocation__labels"
                  aria-label="Owner allocation values"
                  start={start + 1}
                >
                  {rows.map((row) => (
                    <li
                      key={row.subject}
                      aria-posinset={row.position}
                      aria-setsize={count === 'all' ? data.total : size}
                      style={{ height: ROW }}
                    >
                      <span className="usage-owner-allocation__name" tabIndex={0} title={row.label}>
                        <span>{row.label}</span>
                        {row.disambiguator && <small>{row.disambiguator}</small>}
                        <span className="usage-owner-allocation__full" role="tooltip">
                          {row.label}
                          {row.disambiguator ? ` · ${row.disambiguator}` : ''}
                        </span>
                      </span>
                      <span className="usage-owner-allocation__sr">
                        {displayQuantity(row.allocated_hours)} {unit};{' '}
                        {displayQuantity(row.sandbox_count)} Sandboxes;{' '}
                        {row.complete ? 'Complete' : 'Partial'} history
                      </span>
                    </li>
                  ))}
                </ol>
              </div>
            </div>
          </div>
          <div className="usage-owner-allocation__axis" aria-hidden="true">
            <ResponsiveContainer width="100%" height={36} minWidth={0}>
              <BarChart
                data={[{ value: max }]}
                layout="vertical"
                margin={{ left: 136, right: 24, top: 0, bottom: 0 }}
                accessibilityLayer={false}
              >
                <XAxis
                  type="number"
                  domain={[0, max]}
                  ticks={ticks}
                  tick={{ fill: 'var(--dsw-alias-label-secondary)', fontSize: 12 }}
                  tickLine={false}
                  axisLine={false}
                  tickFormatter={(value: number) =>
                    Intl.NumberFormat('en-SG', { notation: 'compact' }).format(value)
                  }
                />
                <YAxis type="category" hide />
                <Bar dataKey="value" fill="transparent" isAnimationActive={false} />
              </BarChart>
            </ResponsiveContainer>
          </div>
        </div>
      )}
      {count === 'all' && data.next && (
        <Button
          variant="outline"
          size="sm"
          disabled={data.pending || !!data.error || !active}
          onClick={data.loadMore}
        >
          {data.pending ? 'Loading more owners…' : 'Load more owners'}
        </Button>
      )}
      {size > 0 && (
        <OwnerAllocationTable
          key={`${metric}:${count}`}
          items={data.items}
          size={size}
          unit={unit}
        />
      )}
      {partial && (
        <p className="usage-metric-coverage">
          Partial history: bars show recorded allocation only, including any extension from the last
          known state. Missing history is excluded.
        </p>
      )}
    </div>
  );
}
