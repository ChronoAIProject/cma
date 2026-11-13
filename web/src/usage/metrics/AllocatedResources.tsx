import type { UsageMetricRow } from '@chronoai/cma-sdk';
import { AllocationHistory } from './AllocationHistory';
import { displayQuantity, metricValue } from './model';

/** Period totals lead; current capacity is a separate, explicitly labelled snapshot. */
export function AllocatedResources({
  capacity,
  period,
}: {
  readonly capacity: UsageMetricRow['allocation'];
  readonly period: UsageMetricRow['allocated_period'];
}) {
  return (
    <section className="usage-allocation-period" aria-label="Allocated resource-hours">
      <div>
        <h3>Allocated resource-hours</h3>
        <p className="usage-section-description">
          Capacity reserved over the selected period. This is the basis for billing.
        </p>
      </div>
      <div className="usage-metric-cards" role="region" aria-label="Allocated capacity now">
        {(
          [
            ['CPU', 'cpu', 'core-hours', period.cpu_core_hours, capacity.cpu_cores],
            ['Memory', 'memory', 'GiB-hours', period.memory_gib_hours, capacity.memory_bytes],
            ['Disk', 'disk', 'GiB-hours', period.disk_gib_hours, capacity.disk_bytes],
          ] as const
        ).map(([label, metric, unit, total, current]) => (
          <dl className="usage-metric-card usage-resource-card" key={metric}>
            <dt>
              {label} <span>({unit})</span>
            </dt>
            <dd>{displayQuantity(total)}</dd>
            <div className="usage-resource-now">
              <dt>Allocated now</dt>
              <dd>
                <span>{metricValue(current, metric)}</span>
              </dd>
            </div>
          </dl>
        ))}
      </div>
      <AllocationHistory value={period} />
    </section>
  );
}
