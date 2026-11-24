import type { RuntimeAttention } from '../shell/runtimeAttention';
import { Alert } from '../ui/Alert';
import { Button } from '../ui/Button';

export function HomeRuntimeAttention({
  attention,
  onReview,
}: {
  readonly attention?: RuntimeAttention;
  readonly onReview?: () => void;
}) {
  if (!attention || attention.freshness === 'unknown' || !attention.failedSandboxes.length)
    return null;
  const count = attention.failedSandboxes.length;
  const failed = `${count} ${count === 1 ? 'sandbox' : 'sandboxes'} couldn’t update ${count === 1 ? 'its' : 'their'} runtime`;
  return (
    <Alert
      tone={attention.freshness === 'fresh' ? 'warning' : 'info'}
      title="Needs you"
      actions={
        onReview ? (
          <Button size="sm" variant="outline" onClick={onReview}>
            Review
          </Button>
        ) : undefined
      }
    >
      <p>{attention.freshness === 'stale' ? `Last known: ${failed}.` : failed}</p>
      {attention.freshness === 'stale' ? <p>Current runtime status is unconfirmed.</p> : null}
    </Alert>
  );
}
