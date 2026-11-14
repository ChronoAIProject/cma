import type { AuthorizedRead } from '../resources/useAuthorizedRead';
import { Button } from '../ui/Button';
import { SkeletonRows } from '../ui/Skeleton';
import { RetryNotice } from './RetryNotice';

export function AgentAccessPending({
  state,
  onRetry,
}: {
  readonly state: AuthorizedRead<unknown>;
  readonly onRetry: () => void;
}) {
  const error = state.status === 'error' ? state : undefined;
  return (
    <section
      className="console__empty"
      role={error?.retryAt ? 'status' : error ? 'alert' : undefined}
    >
      <h1>{error && !error.retryAt ? 'Agent unavailable' : 'Opening Agent'}</h1>
      {error ? (
        <>
          {error.retryAt ? <RetryNotice at={error.retryAt} /> : <p>{error.message}</p>}
          <Button onClick={onRetry} disabled={error.retryAt !== undefined}>
            Check access again
          </Button>
        </>
      ) : (
        <SkeletonRows rows={3} label="Opening Agent" />
      )}
    </section>
  );
}
