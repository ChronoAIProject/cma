import type { UsageMetricReport } from '@cma/client';
import { Button } from '../../ui/Button';
import { IconRefreshOutline16 } from '../../ui/icons/IconRefreshOutline16';
import { displayQuantity } from './model';
import { sandboxCount } from './sandboxCount';

function dateTime(value: number | string) {
  const numeric = Number(value);
  const date = Number.isFinite(numeric) ? new Date(numeric * 1000) : new Date(value);
  return date.toLocaleString('en-SG', {
    timeZone: 'Asia/Singapore',
    day: 'numeric',
    month: 'short',
    hour: '2-digit',
    minute: '2-digit',
  });
}

export function UsageOverviewHeader({
  report,
  generatedAt,
  busy,
  refreshing,
  stale,
  onRefresh,
}: {
  readonly report?: UsageMetricReport;
  readonly generatedAt?: number;
  readonly busy: boolean;
  readonly refreshing?: boolean;
  readonly stale?: boolean;
  readonly onRefresh: () => void;
}) {
  return (
    <header className="usage-overview">
      <div className="usage-report-heading">
        <h2>Allocation overview</h2>
        <div className="usage-overview-actions">
          {report && (
            <p className="usage-report-time">
              As of {dateTime(generatedAt ?? report.as_of)} SGT
              {(busy || refreshing) && ', refreshing…'}
              {!busy && !refreshing && stale && ', refresh deferred'}
            </p>
          )}
          <Button
            variant="outline"
            size="sm"
            icon={<IconRefreshOutline16 />}
            disabled={busy}
            onClick={onRefresh}
          >
            Refresh usage
          </Button>
        </div>
      </div>
      {report?.summary && (
        <div className="usage-overview-meta">
          <p>
            {sandboxCount(report.summary.sandbox_count)} ·{' '}
            {displayQuantity(report.summary.running_count)} running
          </p>
          <p>
            Allocation period: {dateTime(report.effective_range.from)} –{' '}
            {dateTime(report.effective_range.to)} SGT.
          </p>
        </div>
      )}
    </header>
  );
}
