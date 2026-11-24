import type { ActivityConsumer, ActivityDelivery } from './useChannelActivity';
import { activityStateLabels, ActivityTimestamp } from './ChannelActivityStatus';
import { deliveryLabel, deliveryTime } from './activityDelivery';
import { IconCheckOutline16, IconQuestionOutline14, IconWarningOutline16 } from '../ui/icons';

function ReceiptIcon({ tone }: { readonly tone: 'confirmed' | 'unknown' | 'failed' }) {
  return (
    <span
      className={`channel-activity__fact-icon channel-activity__fact-icon--${tone}`}
      aria-hidden="true"
    >
      {tone === 'confirmed' ? (
        <IconCheckOutline16 />
      ) : tone === 'failed' ? (
        <IconWarningOutline16 />
      ) : (
        <IconQuestionOutline14 />
      )}
    </span>
  );
}

function deliveryTone(delivery: ActivityDelivery) {
  switch (delivery.outcome) {
    case 'confirmed':
    case 'delivered':
    case 'read':
    case 'played':
      return 'confirmed';
    case 'rejected':
    case 'failed':
    case 'unsupported':
    case 'unavailable':
      return 'failed';
    default:
      return 'unknown';
  }
}

export function ChannelReceiptFacts({ consumer }: { readonly consumer: ActivityConsumer }) {
  const produced =
    consumer.state === 'produced' ||
    [consumer.produced_at, consumer.produced_by].some(
      (value) => value && Number.isFinite(Date.parse(value)),
    );
  const name = consumer.display_name ?? consumer.consumer_id;
  return (
    <section
      className="channel-activity__consumer"
      aria-label={`Response and delivery for ${name}`}
    >
      <div className="channel-activity__fact">
        <ReceiptIcon tone={produced ? 'confirmed' : 'unknown'} />
        <div>
          <strong>{produced ? 'Agent response produced' : 'Agent response not confirmed'}</strong>
          <p>
            {name} · <span>{activityStateLabels[consumer.state] ?? consumer.state}</span>
          </p>
          {produced ? (
            <p>
              {!consumer.produced_at ? 'Produced by ' : null}
              <ActivityTimestamp value={consumer.produced_at ?? consumer.produced_by} />
            </p>
          ) : null}
          {!produced ? <p>No response production time is recorded.</p> : null}
        </div>
      </div>
      {consumer.deliveries.length === 0 ? (
        <div className="channel-activity__unconfirmed channel-activity__fact">
          <ReceiptIcon tone="unknown" />
          <div>
            <strong>Delivery not confirmed</strong>
            <p>No recipient delivery receipt recorded.</p>
            <p>No delivery confirmation is recorded. This doesn’t mean the reply wasn’t sent.</p>
          </div>
        </div>
      ) : (
        <ul className="channel-activity__deliveries" aria-label={`Delivery receipts for ${name}`}>
          {consumer.deliveries.map((delivery, index) => (
            <li
              key={`${delivery.step}:${delivery.observed_at}:${index}`}
              className="channel-activity__fact"
            >
              <ReceiptIcon tone={deliveryTone(delivery)} />
              <div>
                <strong>{deliveryLabel(delivery)}</strong>
                <p>
                  {delivery.step.replaceAll('_', ' ')} ·{' '}
                  {delivery.destination === 'origin'
                    ? 'Original destination'
                    : 'Default destination'}
                </p>
                <p>
                  {delivery.recipients?.length
                    ? delivery.recipients
                        .map(
                          (recipient) =>
                            `${recipient.kind}: ${recipient.id}${recipient.thread_id ? ` · Thread ${recipient.thread_id}` : ''}`,
                        )
                        .join(', ')
                    : 'Recipient not reported'}
                </p>
                <ActivityTimestamp value={deliveryTime(delivery)} />
              </div>
            </li>
          ))}
        </ul>
      )}
      <details className="channel-activity__details">
        <summary>Response details</summary>
        <dl>
          {consumer.refusal_code ? (
            <div>
              <dt>Refusal reason</dt>
              <dd>{consumer.refusal_code}</dd>
            </div>
          ) : null}
          {consumer.last_non_admission_code ? (
            <div>
              <dt>Last admission refusal</dt>
              <dd>{consumer.last_non_admission_code}</dd>
            </div>
          ) : null}
          <div>
            <dt>Dispatch started</dt>
            <dd>
              <ActivityTimestamp value={consumer.dispatched_at} />
            </dd>
          </div>
          <div>
            <dt>Accepted by agent</dt>
            <dd>
              <ActivityTimestamp value={consumer.accepted_at} />
            </dd>
          </div>
          <div>
            <dt>Provider Agent</dt>
            <dd>{consumer.agent_id ?? 'Provider Agent identity was not reported'}</dd>
          </div>
          {consumer.execution_reference ? (
            <div>
              <dt>Provider execution reference</dt>
              <dd>{consumer.execution_reference}</dd>
            </div>
          ) : null}
          {consumer.provider ? (
            <div>
              <dt>Provider</dt>
              <dd>{consumer.provider}</dd>
            </div>
          ) : null}
          {consumer.conversation_id ? (
            <div>
              <dt>Agent conversation</dt>
              <dd>{consumer.conversation_id}</dd>
            </div>
          ) : null}
          <div>
            <dt>
              {!consumer.produced_at && consumer.produced_by
                ? 'Response produced by'
                : 'Response produced'}
            </dt>
            <dd>
              <ActivityTimestamp value={consumer.produced_at ?? consumer.produced_by} />
              {!consumer.produced_at && consumer.produced_by ? (
                <span> · Confirmed answer delivery</span>
              ) : null}
            </dd>
          </div>
        </dl>
      </details>
    </section>
  );
}
