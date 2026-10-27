import { Presence } from '../ui/Presence';
import {
  ActivityConnection,
  ActivityTimestamp,
  activityStateLabels,
} from './ChannelActivityStatus';
import type { ChannelActivityFeed } from './useChannelActivity';
import './ChannelActivity.css';

export function ChannelActivityPreview({ feed }: { readonly feed: ChannelActivityFeed }) {
  const latest = feed.page?.items.slice(0, 3) ?? [];
  return (
    <section className="channel-activity-preview" aria-label="Latest channel activity">
      <header>
        <strong>Latest activity</strong>
        <ActivityConnection connection={feed.connection} retry={feed.retry} />
      </header>
      {latest.length ? (
        latest.map((event) => (
          <Presence key={event.event_id}>
            <div className="channel-activity-preview__receipt">
              <p>
                <strong>{event.source.platform ?? event.source.type}</strong>
                <span>{event.actor.display_name ?? event.actor.id}</span>
                <span>
                  Received <ActivityTimestamp value={event.received_at} />
                </span>
              </p>
              {event.consumers.length ? (
                <ul aria-label="Reported consumer states">
                  {event.consumers.map((consumer) => (
                    <li key={consumer.consumer_id}>
                      {consumer.display_name ?? consumer.consumer_id} ·{' '}
                      {activityStateLabels[consumer.state] ?? consumer.state}
                    </li>
                  ))}
                </ul>
              ) : (
                <p>Received</p>
              )}
            </div>
          </Presence>
        ))
      ) : (
        <p className="channel-activity-preview__empty">
          {feed.page
            ? `No events recorded in the last ${feed.page.retention_days} days.`
            : 'Waiting for activity snapshot.'}
        </p>
      )}
    </section>
  );
}
