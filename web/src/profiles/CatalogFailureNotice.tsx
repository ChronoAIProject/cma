import { useEffect, useState } from 'react';
import { Button } from '../ui/Button';
import type { CatalogFailure } from './catalogFailure';

export function CatalogFailureNotice({
  failure,
  onRetry,
  disabled,
}: {
  readonly failure: CatalogFailure;
  readonly onRetry: () => void;
  readonly disabled: boolean;
}) {
  const [delay, setDelay] = useState({ failure, ready: failure.retryAfterMs === undefined });
  if (delay.failure !== failure) {
    setDelay({ failure, ready: failure.retryAfterMs === undefined });
  }
  useEffect(() => {
    if (failure.retryAfterMs === undefined) return;
    const deadline = Date.now() + failure.retryAfterMs;
    let timer: ReturnType<typeof setTimeout>;
    const check = () => {
      const remaining = deadline - Date.now();
      if (remaining <= 0) setDelay({ failure, ready: true });
      else timer = setTimeout(check, Math.min(remaining, 2 ** 31 - 1));
    };
    check();
    return () => clearTimeout(timer);
  }, [failure]);
  return (
    <div role="alert">
      <p>{failure.message}</p>
      {failure.status !== undefined && (
        <p>
          HTTP {failure.status}
          {failure.code === undefined ? '' : ` · ${failure.code}`}
        </p>
      )}
      {failure.requestId !== undefined && <p>Request ID: {failure.requestId}</p>}
      {failure.retryable && (
        <>
          {!delay.ready && <p>Please wait before retrying.</p>}
          <Button type="button" disabled={disabled || !delay.ready} onClick={onRetry}>
            Try again
          </Button>
        </>
      )}
    </div>
  );
}
