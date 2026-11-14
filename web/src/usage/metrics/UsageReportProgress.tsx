import { useEffect, useState } from 'react';
import { Alert } from '../../ui/Alert';
import type { Range } from './model';

export const usageRangeLabel: Record<Range, string> = {
  '1h': 'Last hour',
  '24h': 'Last 24 hours',
  '7d': 'Last 7 days',
  '30d': 'Last 30 days',
  custom: 'Custom range',
};

export function UsageReportProgress({
  range,
  previewRange,
  busy,
  pending,
  retrying,
  startedAt,
  nextCheckAt,
  active,
}: {
  readonly range: Range;
  readonly previewRange?: Range;
  readonly busy: boolean;
  readonly pending: boolean;
  readonly retrying: boolean;
  readonly startedAt: number;
  readonly nextCheckAt: number;
  readonly active: boolean;
}) {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    if (!busy || !active) return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [active, busy]);
  if (!busy && !previewRange) return null;
  const elapsed = startedAt ? Math.max(0, Math.floor((now - startedAt) / 1000)) : 0;
  const next = Math.max(0, Math.ceil((nextCheckAt - now) / 1000));
  return (
    <Alert
      tone="info"
      title={
        previewRange
          ? `Showing ${usageRangeLabel[previewRange]}`
          : retrying
            ? 'Retrying usage automatically…'
            : pending
              ? 'Preparing usage report…'
              : 'Loading usage report…'
      }
    >
      {previewRange && (
        <p>
          This is the last committed {usageRangeLabel[previewRange].toLowerCase()} report, not{' '}
          {usageRangeLabel[range].toLowerCase()}. Its actual period and As-of time are shown in the
          report header.
        </p>
      )}
      {busy && (
        <>
          <p>
            {usageRangeLabel[range]}:{' '}
            {retrying
              ? 'temporarily unavailable; retrying automatically'
              : 'waiting for the complete report'}
            . {elapsed}s elapsed.
          </p>
          <p>
            {!active
              ? 'Checks pause while this view is hidden.'
              : next > 0
                ? `Next check in about ${next}s.`
                : 'Checking now…'}{' '}
            A completion estimate is not available. Automatic checks pause after two minutes.
          </p>
        </>
      )}
    </Alert>
  );
}
