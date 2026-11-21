import './AllocationHistory.css';
import type { UsageMetricRow } from '@cma/client';
import { Badge } from '../../ui/Badge';
import { allocationCoverage } from './allocationCoverage';
import { humanDuration } from './humanDuration';

export function AllocationHistory({
  value,
}: {
  readonly value: UsageMetricRow['allocated_period'];
}) {
  const history = allocationCoverage(value);
  return (
    <details className="usage-allocation-history">
      <summary>
        <Badge tone={history.coverage === 'complete' ? 'neutral' : 'warning'}>
          {history.label}
        </Badge>
        {history.coverage === 'missing' && (
          <span>Confirmed missing duration: {history.missingDuration}</span>
        )}
        {history.coverage === 'unverified' && <span>{history.description}</span>}
        {history.projection && <span>{history.projection}</span>}
        <span className="usage-history-disclosure">How this is calculated</span>
      </summary>
      <p>
        Resource-hours add up the capacity reserved for each Sandbox over time. One CPU core
        allocated for one hour is one core-hour. Retained disk continues accruing allocated time
        while a Sandbox is paused.
      </p>
      <dl className="usage-history-facts">
        <div>
          <dt>Missing intervals</dt>
          <dd>{history.missingDuration}</dd>
        </div>
        <div>
          <dt>Projection from last known state</dt>
          <dd>
            {humanDuration(value.projected_tail_seconds)}
            {history.projected ? ' (estimated)' : ''}
          </dd>
        </div>
      </dl>
      {history.coverage !== 'unverified' && <p>{history.description}</p>}
      {history.projected && (
        <p>
          Projection estimates allocation from the last known state to the end of the period until
          another lifecycle event is recorded. It does not establish missing intervals or verify
          coverage.
        </p>
      )}
      <p>Durations add up across Sandboxes and can exceed the report period.</p>
    </details>
  );
}
