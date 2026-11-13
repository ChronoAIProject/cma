import type { UsageMetricReport, UsageMetricRow, UsageMetricQuantity } from '@chronoai/cma-sdk';
import { AllocatedResources } from './AllocatedResources';
import { AllocationHistory } from './AllocationHistory';
import { Button } from '../../ui/Button';
import { displayQuantity, metricLabels, metricTime, metricValue, type Metric } from './model';

type Measurement = UsageMetricRow['measured'][Metric];
export function MetricReading({
  metric,
  reading,
  capacity,
}: {
  readonly metric: Metric;
  readonly reading?: Measurement | null;
  readonly capacity: UsageMetricQuantity;
}) {
  const latest = reading
    ? 'latest_cores' in reading
      ? reading.latest_cores
      : reading.latest_bytes
    : null;
  const mean = reading ? ('mean_cores' in reading ? reading.mean_cores : reading.mean_bytes) : null;
  return (
    <>
      <p>
        <strong>{metricValue(latest, metric)}</strong> measured / {metricValue(capacity, metric)}{' '}
        allocated now
      </p>
      <p>Period mean: {metricValue(mean, metric)}</p>
      {reading && (
        <p>
          Measured period:{' '}
          {displayQuantity(
            'observed_core_hours' in reading
              ? reading.observed_core_hours
              : reading.observed_gib_hours,
          )}{' '}
          {metric === 'cpu' ? 'core-hours' : 'GiB-hours'}.
        </p>
      )}
      <Coverage metric={metric} reading={reading} />
      {latest === null && reading?.last_observed != null && (
        <p>
          Last observed: {metricValue(reading.last_observed, metric)} at{' '}
          {metricTime(reading.observed_at)}
        </p>
      )}
      {metric === 'disk' && reading?.filesystem_capacity_bytes != null && (
        <p>Filesystem capacity: {metricValue(reading.filesystem_capacity_bytes, metric)}</p>
      )}
      {reading?.percent_of_limit != null && (
        <p>
          Matching measured capacity: {displayQuantity(reading.percent_of_limit)}%
          {reading.exceeds_capacity ? ' (exceeds capacity)' : ''}
        </p>
      )}
    </>
  );
}

export function Coverage({
  metric,
  reading,
}: {
  readonly metric: Metric;
  readonly reading?: Measurement | null;
}) {
  return (
    <div className="usage-metric-coverage">
      <p>
        Measured coverage:{' '}
        {reading?.coverage_ratio == null
          ? 'Unavailable'
          : `${displayQuantity(Number(reading.coverage_ratio) * 100)}%`}{' '}
        · {reading?.state.replaceAll('_', ' ') ?? 'unavailable'}
      </p>
      {reading && (
        <>
          <p>
            {displayQuantity(reading.covered_seconds)} / {displayQuantity(reading.expected_seconds)}{' '}
            seconds observed; {displayQuantity(reading.unknown_mapping_seconds)} seconds with
            unknown mapping.
          </p>
          <p>
            {metricLabels[metric]} observed at {metricTime(reading.observed_at)}; source{' '}
            {reading.source}.
          </p>
        </>
      )}
    </div>
  );
}

export function AllocatedPeriod({ value }: { readonly value: UsageMetricRow['allocated_period'] }) {
  return (
    <section className="usage-allocation-period" aria-label="Allocated resource-hours">
      <h3>Allocated resource-hours</h3>
      <p className="usage-section-description">
        Capacity reserved over the selected period. This is the basis for billing.
      </p>
      <dl className="usage-metric-totals">
        <div>
          <dt>CPU (core-hours)</dt>
          <dd>{displayQuantity(value.cpu_core_hours)}</dd>
        </div>
        <div>
          <dt>Memory (GiB-hours)</dt>
          <dd>{displayQuantity(value.memory_gib_hours)}</dd>
        </div>
        <div>
          <dt>Disk (GiB-hours)</dt>
          <dd>{displayQuantity(value.disk_gib_hours)}</dd>
        </div>
      </dl>
      <AllocationHistory value={value} />
    </section>
  );
}

export function MetricSummary({
  report,
  measured,
  selectedMetric,
  onMetric,
}: {
  readonly report: UsageMetricReport;
  readonly measured: boolean;
  readonly selectedMetric?: Metric;
  readonly onMetric?: (metric: Metric) => void;
}) {
  const summary = report.summary;
  if (!summary) return null;
  return (
    <>
      {measured && (
        <>
          <p>
            Measured through {metricTime(report.measurement_cutoff)}. Source health:{' '}
            {report.source_health}. Tracking started {metricTime(report.tracking_started_at)};
            measurements available from {metricTime(report.available_from)}.
          </p>
          {report.warnings.length > 0 && (
            <ul aria-label="Usage report coverage notices">
              {report.warnings.map((warning) => (
                <li key={warning}>{warning}</li>
              ))}
            </ul>
          )}
          {!report.report_complete && (
            <p role="status">
              This report is incomplete. Review coverage and allocation-history gaps.
            </p>
          )}
          <div className="usage-metric-cards">
            {(['cpu', 'memory', 'disk'] as const).map((metric) => (
              <section className="usage-metric-card" key={metric} aria-label={metricLabels[metric]}>
                <h3>
                  {onMetric ? (
                    <Button
                      aria-pressed={metric === selectedMetric}
                      onClick={() => onMetric(metric)}
                    >
                      {metricLabels[metric]}
                    </Button>
                  ) : (
                    metricLabels[metric]
                  )}
                </h3>
                <MetricReading
                  metric={metric}
                  reading={summary.measured?.[metric]}
                  capacity={
                    summary.total_allocated_capacity[
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
        </>
      )}
      <AllocatedResources
        capacity={summary.total_allocated_capacity}
        period={summary.allocated_period}
      />
      {measured && report.scope.admin && summary.infrastructure && (
        <details>
          <summary>Infrastructure diagnostics (excluded from owner totals)</summary>
          <p>
            {summary.infrastructure.state}; {displayQuantity(summary.infrastructure.unproven_count)}{' '}
            identities unproven. Observed {metricTime(summary.infrastructure.observed_at)}.
          </p>
          {(['cpu', 'memory', 'disk'] as const).map((metric, index) => (
            <p key={metric}>
              {metricLabels[metric]}: unassigned pool{' '}
              {metricValue(summary.infrastructure!.unassigned_pool[index], metric)}; unattributed{' '}
              {metricValue(summary.infrastructure!.unattributed[index], metric)}.
            </p>
          ))}
        </details>
      )}
    </>
  );
}

export function AllocatedCapacity({ value }: { readonly value: UsageMetricRow['allocation'] }) {
  return (
    <section className="usage-allocation-capacity" aria-label="Allocated capacity now">
      <h3>Allocated capacity now</h3>
      <div className="usage-metric-cards">
        {(
          [
            ['CPU', value.cpu_cores, 'cpu'],
            ['Memory', value.memory_bytes, 'memory'],
            ['Disk', value.disk_bytes, 'disk'],
          ] as const
        ).map(([label, amount, metric]) => (
          <dl className="usage-metric-card" key={metric}>
            <dt>{label} allocated</dt>
            <dd>{metricValue(amount, metric)}</dd>
          </dl>
        ))}
      </div>
    </section>
  );
}
