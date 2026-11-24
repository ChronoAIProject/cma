import { useState, type ReactNode } from 'react';
import type { UsageMetricRow } from '@cma/client';
import { StatusBadge } from '../../ui/Badge';
import { Button } from '../../ui/Button';
import { TableCell } from '../../ui/Table';
import { UsageIdentity } from './UsageIdentity';
import { AllocationCoverageStatus } from './AllocationCoverageStatus';
import { displayQuantity, metricTime, metricValue, type Metric } from './model';

export function UsageInventoryRow({
  row,
  admin,
  measured,
  onDetail,
  actions,
}: {
  readonly row: UsageMetricRow;
  readonly admin: boolean;
  readonly measured: boolean;
  readonly onDetail: (row: UsageMetricRow) => void;
  readonly actions?: (row: UsageMetricRow) => ReactNode;
}) {
  const [metadataOpen, setMetadataOpen] = useState(false);
  return (
    <tr>
      <TableCell>
        <div className="usage-inventory-name">
          <Button
            size="sm"
            placement="usage-inventory-name__button"
            title={row.name || 'Unnamed Sandbox'}
            onClick={() => onDetail(row)}
          >
            {row.name || 'Unnamed Sandbox'}
          </Button>
          <StatusBadge status={row.status} />
        </div>
        <div className="usage-inventory-secondary">
          {admin && (
            <span className="usage-metric-id" title={row.owner.subject}>
              {row.owner.display_name || 'Owner name unavailable'}
            </span>
          )}
          <details
            className="usage-inventory-meta"
            onToggle={(event) => setMetadataOpen(event.currentTarget.open)}
          >
            <summary>
              Details · {displayQuantity(row.agent_count)}{' '}
              {row.agent_count === 1 ? 'Agent' : 'Agents'}
            </summary>
            {metadataOpen && (
              <>
                <UsageIdentity label="Sandbox ID" value={row.sandbox_id} />
                {admin && <UsageIdentity label="Owner ID" value={row.owner.subject} />}
                <span className="usage-metric-id">Runtime image</span>
                {row.runtime_version ? (
                  <UsageIdentity label="runtime image" value={row.runtime_version} />
                ) : (
                  <span>Not bound</span>
                )}
                <span className="usage-metric-id">
                  Last activity: {metricTime(row.last_activity_at)}
                </span>
              </>
            )}
          </details>
        </div>
        <AllocationCoverageStatus value={row.allocated_period} />
        {actions && <div className="usage-metric-actions">{actions(row)}</div>}
      </TableCell>
      {(['cpu', 'memory', 'disk'] as const).map((metric) => (
        <TableCell key={metric} align="end">
          <span className="usage-inventory-mobile-label">
            {metric === 'cpu' ? 'CPU' : metric === 'memory' ? 'Memory' : 'Disk'}
          </span>
          <strong className="usage-inventory-quantity">
            {displayQuantity(
              row.allocated_period[
                metric === 'cpu'
                  ? 'cpu_core_hours'
                  : metric === 'memory'
                    ? 'memory_gib_hours'
                    : 'disk_gib_hours'
              ],
            )}
          </strong>
          <span className="usage-inventory-mobile-unit">
            {metric === 'cpu' ? 'core-hours' : 'GiB-hours'}
          </span>
          <span className="usage-metric-id">
            {metricValue(
              row.allocation[
                metric === 'cpu' ? 'cpu_cores' : metric === 'memory' ? 'memory_bytes' : 'disk_bytes'
              ],
              metric,
            )}{' '}
            now
          </span>
          {measured && <CompactReading row={row} metric={metric} />}
        </TableCell>
      ))}
    </tr>
  );
}

function CompactReading({
  row,
  metric,
}: {
  readonly row: UsageMetricRow;
  readonly metric: Metric;
}) {
  const reading = row.measured[metric];
  const value =
    metric === 'cpu'
      ? row.measured.cpu.mean_cores
      : metric === 'memory'
        ? row.measured.memory.mean_bytes
        : row.measured.disk.latest_bytes;
  return (
    <div className="usage-inventory-measured">
      <span>Measured: {metricValue(value, metric)}</span>
      <span className="usage-metric-id">
        {reading.state.replaceAll('_', ' ')} · coverage{' '}
        {reading.coverage_ratio == null
          ? 'unavailable'
          : `${displayQuantity(Number(reading.coverage_ratio) * 100)}%`}
      </span>
      <span className="usage-metric-id">Observed {metricTime(reading.observed_at)}</span>
      {value === null && reading.last_observed !== null && (
        <span className="usage-metric-id">
          Last observed {metricValue(reading.last_observed, metric)}
        </span>
      )}
    </div>
  );
}
