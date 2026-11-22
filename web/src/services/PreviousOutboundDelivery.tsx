import { useEffect, useRef, useState } from 'react';
import { Alert } from '../ui/Alert';
import { Button } from '../ui/Button';
import { OutboundMediaReview } from './OutboundMediaFields';
import type { OutboundIntent } from './outboundJournal';
import { outboundStatus, validOutboundReceipt } from './outboundStatus';
import { segment, serviceRequest, ServiceError, type Denied } from './client';

export function PreviousOutboundDelivery({
  base,
  intent,
  retain,
  denied,
}: {
  base: string;
  intent: OutboundIntent;
  retain: (intent?: OutboundIntent) => boolean;
  denied: Denied;
}) {
  const [pending, setPending] = useState(false);
  const [error, setError] = useState('');
  const active = useRef<AbortController | undefined>(undefined);
  useEffect(() => () => active.current?.abort(), []);
  const receipt = intent.receipt;
  if (!receipt) return null;
  const status = outboundStatus(receipt, intent.uncertain, intent.preparationExpired);
  async function check() {
    if (active.current || !receipt) return;
    const abort = new AbortController();
    active.current = abort;
    setPending(true);
    setError('');
    try {
      const value = await serviceRequest(
        'event-gateway',
        `${base}/${segment(receipt.delivery_id)}`,
        {
          method: 'GET',
          signal: AbortSignal.any([abort.signal, AbortSignal.timeout(15000)]),
        },
      );
      if (abort.signal.aborted) return;
      if (
        !validOutboundReceipt(value) ||
        value.delivery_id !== receipt.delivery_id ||
        value.route_id !== intent.draft.body.route_id
      )
        throw new Error('Unverified delivery');
      retain({ ...intent, receipt: value, uncertain: value.delivery?.outcome === 'unknown' });
    } catch (cause) {
      if (abort.signal.aborted) return;
      if (cause instanceof ServiceError && cause.status === 401) denied(cause.status);
      setError(
        'The previous delivery status could not be verified. Its original receipt is retained.',
      );
    } finally {
      active.current = undefined;
      if (!abort.signal.aborted) setPending(false);
    }
  }
  return (
    <section className="channel-form-section" aria-label="Previous unconfirmed delivery">
      <h4>Previous unconfirmed delivery</h4>
      <p>{intent.draft.body.message.text}</p>
      <OutboundMediaReview items={intent.draft.body.message.native_attachments ?? []} />
      <p role="status">{status.message}</p>
      <p className="channel-field-hint">Delivery reference: {receipt.delivery_id}</p>
      {error ? <Alert tone="warning">{error}</Alert> : null}
      {status.check ? (
        <Button disabled={pending} onClick={() => void check()}>
          Check previous delivery status
        </Button>
      ) : null}
      {status.newMessage ? (
        <Button disabled={pending} onClick={() => retain()}>
          Dismiss previous delivery
        </Button>
      ) : null}
    </section>
  );
}
