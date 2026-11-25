import { QuotedContextSummary } from './QuotedContextSummary';
import { Presence } from '../ui/Presence';
import { IconCheckOutline16 } from '../ui/icons';
import './ChannelActivity.css';
import { useChannelActivity, type ChannelActivityFeed } from './useChannelActivity';
import { activityActor, activityLabel } from './channelActivityLabels';
import { ActivityConnection, ActivityTimestamp } from './ChannelActivityStatus';
import { ChannelReceiptFacts } from './ChannelReceiptFacts';

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
      <div className="channel-activity__heading">
        <h3>Conversation activity</h3>
        <p>Recorded arrival, agent response and delivery status for the original event.</p>
      </div>
      <ActivityConnection connection={connection} retry={retry} refresh />
      {!page ? (
        <p className="channel-activity__notice">
          Activity records haven’t loaded. No event or delivery status is confirmed by this view.
        </p>
      ) : null}
      {page?.items.length === 0 ? (
        <p className="channel-activity__notice">
          No events recorded in the last {page.retention_days} days.
        </p>
      ) : null}
      <ol className="channel-activity__events">
        {page?.items.map((event) => (
          <li key={event.event_id}>
            <Presence>
              <article className="channel-activity__event">
                <header>
                  <div>
                    <p>
                      {activityActor(event)} · {event.source.platform ?? event.source.type}
                    </p>
                    <strong>
                      {activityLabel(event)} · {event.source.platform ?? event.source.type}
                    </strong>
                  </div>
                  <span className="channel-activity__badge">Activity recorded</span>
                </header>
                <QuotedContextSummary context={event.quoted_context} />
                <div className="channel-activity__fact channel-activity__arrival">
                  <span
                    className="channel-activity__fact-icon channel-activity__fact-icon--confirmed"
                    aria-hidden="true"
                  >
                    <IconCheckOutline16 />
                  </span>
                  <div>
                    <strong>Received</strong>
                    <p>
                      The original event is recorded ·{' '}
                      <ActivityTimestamp value={event.received_at} />
                    </p>
                  </div>
                </div>
                {event.consumers.length === 0 ? (
                  <p className="channel-activity__notice">
                    No Agent response or recipient delivery receipt is recorded.
                  </p>
                ) : null}
                {event.consumers.map((consumer) => (
                  <ChannelReceiptFacts key={consumer.consumer_id} consumer={consumer} />
                ))}
                <footer className="channel-activity__event-footer">
                  <p>
                    Original event · <code>{event.event_id}</code>
                  </p>
                  <details className="channel-activity__details">
                    <summary>Event details</summary>
                    <dl>
                      <div>
                        <dt>
                          {event.kind === 'event' ? 'Source event time' : 'Recorded event time'}
                        </dt>
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
                  </details>
                </footer>
              </article>
            </Presence>
          </li>
        ))}
      </ol>
      <p className="channel-activity__notice">
        An agent’s reply isn’t proof the other side received it.
      </p>
      {page?.has_more ? (
        <p className="channel-field-hint">
          Showing {page.items.length} recent events (up to {page.limit}; retained for{' '}
          {page.retention_days} days).
        </p>
      ) : null}
    </div>
  );
}
