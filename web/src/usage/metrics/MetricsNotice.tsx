import { useEffect, useState } from 'react';
import { Alert } from '../../ui/Alert';
import { Button } from '../../ui/Button';
import type { MetricsFailure } from './failure';

export function MetricsNotice({
  failure,
  onRetry,
  retryLabel = 'Retry usage',
}: {
  readonly failure?: MetricsFailure;
  readonly onRetry: () => void;
  readonly retryLabel?: string;
}) {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    if (!failure?.retryAt) return;
    const timer = setTimeout(
      () => setNow(Date.now()),
      Math.max(0, Math.min(2147483647, failure.retryAt - Date.now())),
    );
    return () => clearTimeout(timer);
  }, [failure]);
  if (!failure) return null;
  const waiting = failure.retryAt !== undefined && failure.retryAt > now;
  return (
    <Alert tone="error" title={failure.title}>
      <p>{failure.message}</p>
      <Button variant="outline" size="sm" disabled={waiting} onClick={onRetry}>
        {waiting ? 'Waiting to retry' : failure.expired ? 'Refresh usage report' : retryLabel}
      </Button>
    </Alert>
  );
}
