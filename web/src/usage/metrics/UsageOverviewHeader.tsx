import type { UsageMetricReport } from '@chronoai/cma-sdk';
import { Button } from '../../ui/Button';
import { IconRefreshOutline16 } from '../../ui/icons/IconRefreshOutline16';
import { displayQuantity } from './model';

function dateTime(value: number | string) {
  return new Date(Number(value) * 1000).toLocaleString('en-SG', {
    timeZone: 'Asia/Singapore',
    day: 'numeric',
    month: 'short',
    hour: '2-digit',
    minute: '2-digit',
  });
}

export function UsageOverviewHeader({
  report,
  busy,
  onRefresh,
}: {
  readonly report?: UsageMetricReport;
  readonly busy: boolean;
  readonly onRefresh: () => void;
}) {
  return (
    <header className="usage-overview">
      <div className="usage-report-heading">
        <h2>Allocation overview</h2>
        <div className="usage-overview-actions">
          {report && <p className="usage-report-time">Updated {dateTime(report.as_of)} SGT</p>}
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
            {displayQuantity(report.summary.sandbox_count)} Sandboxes ·{' '}
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
