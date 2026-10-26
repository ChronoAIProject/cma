import { Button } from '../ui/Button';
import { Presence } from '../ui/Presence';
import './ChannelActivity.css';
import { useChannelActivity, type ActivityConsumer } from './useChannelActivity';

function Timestamp({ value }: { readonly value?: string | null }) {
  if (!value || !Number.isFinite(Date.parse(value))) return <span>Not reported</span>;
  return (
    <time dateTime={value} title={new Date(value).toLocaleString()}>
      {new Date(value).toLocaleString([], {
        month: 'short',
        day: 'numeric',
        hour: '2-digit',
        minute: '2-digit',
        second: '2-digit',
      })}
    </time>
  );
}
const labels: Record<ActivityConsumer['state'], string> = {
  received: 'Received',
  queued: 'Queued',
  dispatching: 'Dispatching to provider',
  accepted: 'Accepted by agent',
  produced: 'Response produced',
  completed: 'Agent completed',
  refused: 'Refused',
  failed: 'Agent response failed',
  unknown: 'Outcome unknown',
};
export function ChannelActivity({
  channelId,
  owner,
  active,
}: {
  readonly channelId: string;
  readonly owner: string;
  readonly active: boolean;
}) {
  const { page, connection, retry } = useChannelActivity(channelId, owner, active);
  return (
    <div className="channel-activity">
      <div className="channel-activity__connection">
        <span role="status">
          {connection === 'live'
            ? 'Live activity'
            : connection === 'connecting'
              ? 'Connecting to live activity…'
              : connection === 'reconnecting'
                ? 'Reconnecting live activity…'
                : 'Live activity disconnected'}
        </span>
        {connection === 'disconnected' ? (
          <Button size="sm" onClick={retry}>
            Reconnect activity
          </Button>
        ) : null}
      </div>
      {page?.items.length === 0 ? (
        <p>No events recorded in the last {page.retention_days} days.</p>
      ) : null}
      <ol className="channel-activity__events">
        {page?.items.map((event) => (
          <li key={event.event_id}>
            <Presence>
              <article className="channel-activity__event">
                <header>
                  <strong>{event.source.platform ?? event.source.type}</strong>
                  <span>
                    Received <Timestamp value={event.received_at} />
                  </span>
                </header>
                <p>
                  {event.actor.display_name ?? event.actor.id} · <code>{event.event_id}</code>
                </p>
                <dl>
                  <div>
                    <dt>Event time</dt>
                    <dd>
                      <Timestamp value={event.occurred_at} />
                    </dd>
                  </div>
                  <div>
                    <dt>Conversation</dt>
                    <dd>{event.conversation.id}</dd>
                  </div>
                </dl>
                {event.consumers.map((consumer) => (
                  <section key={consumer.consumer_id} className="channel-activity__consumer">
                    <header>
                      <strong>{consumer.display_name ?? consumer.consumer_id}</strong>
                      <span>{labels[consumer.state] ?? consumer.state}</span>
                    </header>
                    <dl>
                      <div>
                        <dt>Dispatch started</dt>
                        <dd>
                          <Timestamp value={consumer.dispatched_at} />
                        </dd>
                      </div>
                      <div>
                        <dt>Accepted by agent</dt>
                        <dd>
                          <Timestamp value={consumer.accepted_at} />
                        </dd>
                      </div>
                      <div>
                        <dt>Produced by</dt>
                        <dd>{consumer.agent_id ?? 'Not reported'}</dd>
                      </div>
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
                        <dt>Response produced</dt>
                        <dd>
                          <Timestamp value={consumer.produced_at} />
                        </dd>
                      </div>
                    </dl>
                    {consumer.deliveries.length === 0 ? (
                      <p>No recipient delivery receipt recorded.</p>
                    ) : (
                      <ul>
                        {consumer.deliveries.map((delivery, index) => (
                          <li key={`${delivery.step}:${delivery.observed_at}:${index}`}>
                            <span>
                              {delivery.step.replaceAll('_', ' ')} ·{' '}
                              {delivery.recipients?.length
                                ? delivery.recipients
                                    .map(
                                      (recipient) =>
                                        `${recipient.kind}: ${recipient.id}${recipient.thread_id ? ` · Thread ${recipient.thread_id}` : ''}`,
                                    )
                                    .join(', ')
                                : 'Recipient not reported'}
                            </span>
                            <strong>
                              {delivery.outcome === 'confirmed'
                                ? 'Delivery confirmed'
                                : delivery.outcome === 'buffered'
                                  ? 'Buffered locally'
                                  : delivery.outcome === 'unavailable'
                                    ? 'Delivery unavailable'
                                    : delivery.outcome.replaceAll('_', ' ')}
                            </strong>
                            <Timestamp
                              value={
                                delivery.outcome === 'confirmed'
                                  ? delivery.confirmed_at
                                  : delivery.observed_at
                              }
                            />
                          </li>
                        ))}
                      </ul>
                    )}
                  </section>
                ))}
              </article>
            </Presence>
          </li>
        ))}
      </ol>
      {page?.has_more ? (
        <p className="channel-field-hint">
          Showing {page.items.length} recent events (up to {page.limit}; retained for{' '}
          {page.retention_days} days).
        </p>
      ) : null}
    </div>
  );
}
