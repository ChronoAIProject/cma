import type { UsageMetricRow } from '@cma/client';
import { IconCheckOutline16 } from '../../ui/icons/IconCheckOutline16';
import { IconWarningOutline16 } from '../../ui/icons/IconWarningOutline16';
import { allocationCoverage } from './allocationCoverage';
import { displayQuantity } from './model';
import { humanDuration } from './humanDuration';
import './UsageAllocationOverview.css';

function projectedHours(seconds: UsageMetricRow['allocated_period']['projected_tail_seconds']) {
  const value = BigInt(seconds);
  const thousandths = (value * 1000n + 1800n) / 3600n;
  if (value > 0n && thousandths === 0n) return '<0.001';
  const fraction = (thousandths % 1000n).toString().padStart(3, '0').replace(/0+$/, '') || '0';
  return `${(thousandths / 1000n).toLocaleString('en-SG')}.${fraction}`;
}

export function UsageAllocationOverview({
  value,
}: {
  readonly value: UsageMetricRow['allocated_period'] | null;
}) {
  const history = value ? allocationCoverage(value) : undefined;
  const verified = history?.coverage === 'complete';
  const missing = history?.coverage === 'missing';
  return (
    <section className="usage-allocation-overview" aria-label="Allocation and history overview">
      <div className="usage-history-notice" role="note" aria-label="Report allocation coverage">
        <span className="usage-history-notice__icon" data-verified={verified} aria-hidden="true">
          {verified ? <IconCheckOutline16 size={24} /> : <IconWarningOutline16 size={24} />}
        </span>
        <div>
          <h3>
            {!history
              ? 'History coverage unavailable'
              : verified
                ? 'History coverage verified'
                : missing
                  ? 'History has confirmed gaps'
                  : 'History not fully verified'}
          </h3>
          <p>
            {!history
              ? 'Allocation history is unavailable for this report. Unavailable usage is not zero.'
              : missing || verified
                ? history.description
                : 'We can’t yet confirm this history is complete. That doesn’t mean anything is missing, and a gap is never counted as zero.'}
          </p>
        </div>
      </div>
      <dl className="usage-allocation-overview__cards">
        <div className="usage-allocation-overview__card">
          <dt>Reported allocation</dt>
          <dd>
            {displayQuantity(value?.cpu_core_hours)}
            {value && <span className="usage-allocation-overview__unit"> core-h</span>}
          </dd>
          <dd className="usage-allocation-overview__explanation">
            CPU capacity over time. May include the projected tail.
          </dd>
        </div>
        <div className="usage-allocation-overview__card">
          <dt>Known gaps</dt>
          <dd className="usage-allocation-overview__gap">
            {history?.missingDuration ?? 'Unavailable'}
          </dd>
          <dd className="usage-allocation-overview__explanation">
            {missing
              ? 'Confirmed missing intervals are excluded from totals, not counted as zero.'
              : verified
                ? 'No missing intervals in the verified allocation history.'
                : 'Not verified isn’t the same as missing.'}
          </dd>
        </div>
        <div className="usage-allocation-overview__card">
          <dt>Projected tail</dt>
          <dd title={value ? humanDuration(value.projected_tail_seconds) : undefined}>
            {value ? projectedHours(value.projected_tail_seconds) : 'Unavailable'}
            {value && <span className="usage-allocation-overview__unit"> h</span>}
          </dd>
          <dd className="usage-allocation-overview__explanation">
            {history?.projected === false
              ? 'No projected tail reported. This does not verify coverage.'
              : 'Estimated from the last known state. Does not verify coverage.'}
          </dd>
        </div>
      </dl>
      <p className="usage-allocation-overview__footnote">
        Allocation totals and projected duration use different units. Durations add up across
        Sandboxes.
      </p>
    </section>
  );
}
