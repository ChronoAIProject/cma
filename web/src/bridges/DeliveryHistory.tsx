import { useState } from 'react';
import type {
  BridgeDelivery,
  BridgeDeliveryRecoveryAction,
  BridgeDeliveryStatus,
} from '@chronoai/cma-sdk';
import { Button } from '../ui/Button';

export const deliveryStatus: Readonly<Record<BridgeDeliveryStatus, string>> = {
  pending: 'Queued',
  claimed: 'Preparing delivery',
  sending: 'Delivery in progress',
  retry_wait: 'Waiting to retry',
  sent: 'Confirmed sent',
  rejected: 'Not sent',
  unknown: 'Delivery unknown',
  abandoned: 'Abandoned',
};
export function DeliveryHistory({
  item,
  disabled,
  onRecover,
}: {
  readonly item: BridgeDelivery;
  readonly disabled: boolean;
  readonly onRecover: (action: BridgeDeliveryRecoveryAction) => void;
}) {
  const [accepted, setAccepted] = useState(false);
  const unknown = item.status === 'unknown';
  const retry =
    ['unknown', 'rejected', 'retry_wait'].includes(item.status) && item.attempts.length < 16;
  const abandon = !['sending', 'sent', 'abandoned'].includes(item.status);
  return (
    <li>
      <p>
        <strong>Reply {item.sequence}</strong> · Part {item.chunk_index + 1} of {item.chunk_count} ·{' '}
        {deliveryStatus[item.status]}
      </p>
      <p>{new Date(item.created_at_ms).toLocaleString()}</p>
      {unknown ? (
        <p>The reply may have reached the recipient. Automatic retry is stopped.</p>
      ) : null}
      {item.duplicate_risk ? (
        <p>An earlier uncertain send was retried with duplicate risk accepted.</p>
      ) : null}
      {item.next_retry_at_ms === null ? null : (
        <p>Next attempt: {new Date(item.next_retry_at_ms).toLocaleString()}</p>
      )}
      <details>
        <summary>Attempts and receipt</summary>
        {item.attempts.length === 0 ? (
          <p>No delivery attempts recorded.</p>
        ) : (
          <ol>
            {item.attempts.map((attempt) => (
              <li key={attempt.id}>
                Attempt {attempt.number}: {deliveryStatus[attempt.status]} ·{' '}
                {new Date(attempt.started_at_ms).toLocaleString()}
                {attempt.failure === null ? '' : ` · ${attempt.failure.replaceAll('_', ' ')}`}
              </li>
            ))}
          </ol>
        )}
        <p>
          Receipt: <code>{item.receipt_id}</code>
        </p>
        <p>Original settings version: {item.config_revision}</p>
      </details>
      {!item.upstream_reconciliation_supported && unknown ? (
        <p>
          The connector cannot verify this earlier send. Refreshing its state cannot confirm
          delivery.
        </p>
      ) : null}
      {unknown && retry ? (
        <label className="bridges__check">
          <input
            type="checkbox"
            checked={accepted}
            disabled={disabled}
            onChange={(event) => setAccepted(event.target.checked)}
          />
          I accept that retrying may send a duplicate reply.
        </label>
      ) : null}
      <div className="bridges__actions">
        {retry ? (
          <Button
            disabled={disabled || (unknown && !accepted)}
            onClick={() => onRecover({ kind: 'retry', accept_duplicate_risk: accepted })}
          >
            Retry original reply
          </Button>
        ) : null}
        {abandon ? (
          <Button disabled={disabled} onClick={() => onRecover({ kind: 'abandon' })}>
            Abandon reply
          </Button>
        ) : null}
        <Button disabled={disabled} onClick={() => onRecover({ kind: 'reconcile' })}>
          Check delivery state
        </Button>
      </div>
    </li>
  );
}
