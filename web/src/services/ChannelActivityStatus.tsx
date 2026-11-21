import { Button } from '../ui/Button';
import type { ActivityConsumer, ChannelActivityFeed } from './useChannelActivity';

export function ActivityTimestamp({ value }: { readonly value?: string | null }) {
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

export const activityStateLabels: Record<ActivityConsumer['state'], string> = {
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

export function ActivityConnection({
  connection,
  retry,
}: Pick<ChannelActivityFeed, 'connection' | 'retry'>) {
  return (
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
  );
}
