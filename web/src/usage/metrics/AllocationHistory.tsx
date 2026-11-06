import './AllocationHistory.css';
import type { UsageMetricQuantity, UsageMetricRow } from '@chronoai/cma-sdk';
import { Badge } from '../../ui/Badge';

export function humanDuration(value: UsageMetricQuantity): string {
  const seconds = typeof value === 'string' ? BigInt(value) : BigInt(Math.floor(value));
  if (seconds === 0n) return 'None';
  const days = seconds / 86400n;
  const hours = (seconds % 86400n) / 3600n;
  const minutes = (seconds % 3600n) / 60n;
  const parts = [
    days > 0n ? `${days.toLocaleString('en-SG')}d` : '',
    hours > 0n ? `${hours}h` : '',
    minutes > 0n ? `${minutes}m` : '',
  ].filter(Boolean);
  return parts.length ? parts.slice(0, 2).join(' ') : `${seconds}s`;
}

export function AllocationHistory({
  value,
}: {
  readonly value: UsageMetricRow['allocated_period'];
}) {
  return (
    <details className="usage-allocation-history">
      <summary>
        <Badge tone={value.complete ? 'neutral' : 'warning'}>
          {value.complete ? 'Complete history' : 'Partial history'}
        </Badge>
        <span>
          {value.complete ? 'How allocation is calculated' : 'Some allocation history is missing'}
        </span>
        <span className="usage-history-disclosure">
          {value.complete ? 'Details' : 'What’s missing?'}
        </span>
      </summary>
      <p>
        Resource-hours add up the capacity reserved for each Sandbox over time. One CPU core
        allocated for one hour is one core-hour. Retained disk continues accruing allocated time
        while a Sandbox is paused.
      </p>
      <dl className="usage-history-facts">
        <div>
          <dt>Missing history</dt>
          <dd>{humanDuration(value.unknown_seconds)}</dd>
        </div>
        <div>
          <dt>Extended from last known state</dt>
          <dd>{humanDuration(value.projected_tail_seconds)}</dd>
        </div>
      </dl>
      <p>
        Missing history is excluded from these totals; it does not mean zero usage. The last known
        allocation is extended to the end of the period until another lifecycle event is recorded.
        Durations add up across Sandboxes and can exceed the selected period.
      </p>
    </details>
  );
}
