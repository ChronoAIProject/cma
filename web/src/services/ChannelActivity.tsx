import { QuotedContextSummary } from './QuotedContextSummary';
import { Presence } from '../ui/Presence';
import './ChannelActivity.css';
import { useChannelActivity, type ChannelActivityFeed } from './useChannelActivity';
import { deliveryLabel, deliveryTime } from './activityDelivery';
import { activityActor, activityLabel } from './channelActivityLabels';
import {
  ActivityConnection,
  ActivityTimestamp,
  activityStateLabels,
} from './ChannelActivityStatus';

export function ChannelActivity({
  channelId,
  owner,
  active,
}: {
  readonly channelId: string;
  readonly owner: string;
  readonly active: boolean;
}) {
  const feed = useChannelActivity(channelId, owner, active);
  return <ChannelActivityView feed={feed} />;
}

export function ChannelActivityView({ feed }: { readonly feed: ChannelActivityFeed }) {
  const { page, connection, retry } = feed;
  return (
    <div className="channel-activity">
      <ActivityConnection connection={connection} retry={retry} />
      {page?.items.length === 0 ? (
        <p>No events recorded in the last {page.retention_days} days.</p>
      ) : null}
      <ol className="channel-activity__events">
        {page?.items.map((event) => (
          <li key={event.event_id}>
            <Presence>
              <article className="channel-activity__event">
                <header>
                  <strong>
                    {activityLabel(event)} · {event.source.platform ?? event.source.type}
                  </strong>
                  <span>
                    Received <ActivityTimestamp value={event.received_at} />
                  </span>
                </header>
                <p>
                  {activityActor(event)} · <code>{event.event_id}</code>
                </p>
                <QuotedContextSummary context={event.quoted_context} />
                <dl>
                  <div>
                    <dt>{event.kind === 'event' ? 'Source event time' : 'Recorded event time'}</dt>
                    <dd>
                      <ActivityTimestamp
                        value={
                          event.kind === 'event' ? event.source_occurred_at : event.occurred_at
                        }
                      />
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
                      <span>{activityStateLabels[consumer.state] ?? consumer.state}</span>
                    </header>
                    <dl>
                      {consumer.refusal_code ? (
                        <div>
                          <dt>Refusal reason</dt>
                          <dd>{consumer.refusal_code}</dd>
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
                          <ActivityTimestamp value={consumer.produced_at} />
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
                            <strong>{deliveryLabel(delivery)}</strong>
                            <ActivityTimestamp value={deliveryTime(delivery)} />
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
