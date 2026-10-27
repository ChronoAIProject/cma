import { useEffect, useState } from 'react';
import { segment } from './client';

export interface ActivityDelivery {
  step: 'ack' | 'heartbeat' | 'answer' | 'tool_reply' | 'failure' | 'default_reply';
  destination: 'origin' | 'default';
  outcome:
    | 'confirmed'
    | 'rejected'
    | 'rate_limited'
    | 'unknown'
    | 'unsupported'
    | 'buffered'
    | 'unavailable';
  observed_at: string;
  confirmed_at: string | null;
  platform_message_id: string | null;
  recipients?: { kind: string; id: string; thread_id: string | null }[];
}
export interface ActivityConsumer {
  consumer_id: string;
  display_name?: string | null;
  agent_id?: string | null;
  provider?: string;
  conversation_id?: string;
  state:
    | 'received'
    | 'queued'
    | 'dispatching'
    | 'accepted'
    | 'produced'
    | 'completed'
    | 'refused'
    | 'failed'
    | 'unknown';
  dispatched_at: string | null;
  accepted_at: string | null;
  produced_at: string | null;
  completed_at: string | null;
  response_id: string | null;
  deliveries: ActivityDelivery[];
}
export interface ChannelActivityEvent {
  event_id: string;
  source: { type: string; platform?: string; bot_id?: string; route_id?: string };
  actor: { id: string; display_name?: string; kind: string };
  conversation: { id: string; kind: string; thread_id?: string };
  occurred_at: string;
  received_at: string;
  consumers: ActivityConsumer[];
}
export interface ActivityPage {
  items: ChannelActivityEvent[];
  cursor: string;
  retention_days: number;
  limit: number;
  has_more: boolean;
}

/** The stream's initial snapshot closes the history/live race; no polling or local timestamps. */
export function useChannelActivity(channelId: string, owner: string, active: boolean) {
  const scope = JSON.stringify([owner, channelId]);
  const [revision, setRevision] = useState(0);
  const [previousActive, setPreviousActive] = useState(active);
  const [stored, setStored] = useState<{ scope: string; page: ActivityPage }>();
  const [connection, setConnection] = useState<{
    scope: string;
    status: 'connecting' | 'live' | 'reconnecting' | 'disconnected';
  }>({ scope, status: 'connecting' });
  if (previousActive !== active) {
    setPreviousActive(active);
    if (active) setConnection({ scope, status: 'connecting' });
  }
  useEffect(() => {
    if (!active) return;
    let current = true;
    if (typeof EventSource === 'undefined') return;
    const stream = new EventSource(
      `/api/event-gateway/v1/channels/${segment(channelId)}/activity/stream`,
      { withCredentials: true },
    );
    const receive = (event: MessageEvent<string>) => {
      if (!current) return;
      try {
        const page: ActivityPage = JSON.parse(event.data);
        if (
          !Array.isArray(page.items) ||
          typeof page.cursor !== 'string' ||
          page.items.some(
            (item) =>
              typeof item.event_id !== 'string' ||
              !item.source ||
              !item.actor ||
              !item.conversation ||
              typeof item.conversation.id !== 'string' ||
              !Array.isArray(item.consumers) ||
              item.consumers.some(
                (consumer) =>
                  typeof consumer.consumer_id !== 'string' ||
                  typeof consumer.state !== 'string' ||
                  !Array.isArray(consumer.deliveries) ||
                  consumer.deliveries.some(
                    (delivery) =>
                      typeof delivery.step !== 'string' ||
                      typeof delivery.outcome !== 'string' ||
                      (delivery.recipients != null &&
                        (!Array.isArray(delivery.recipients) ||
                          delivery.recipients.some(
                            (recipient) =>
                              typeof recipient.kind !== 'string' ||
                              typeof recipient.id !== 'string',
                          ))),
                  ),
              ),
          )
        )
          throw new Error('Invalid activity snapshot');
        setStored({ scope, page });
        setConnection({ scope, status: 'live' });
      } catch {
        current = false;
        stream.close();
        setConnection({ scope, status: 'disconnected' });
      }
    };
    stream.addEventListener('activity', receive as EventListener);
    stream.addEventListener('reset', receive as EventListener);
    stream.onerror = () => {
      if (!current) return;
      if (stream.readyState === EventSource.CLOSED) {
        current = false;
        stream.close();
        setConnection({ scope, status: 'disconnected' });
      } else {
        // Native EventSource retains Last-Event-ID across transient reconnects.
        setConnection({ scope, status: 'reconnecting' });
      }
    };
    return () => {
      current = false;
      stream.close();
    };
  }, [channelId, scope, active, revision]);
  return {
    page: stored?.scope === scope ? stored.page : undefined,
    connection: connection.scope === scope ? connection.status : 'connecting',
    retry: () => {
      setConnection({ scope, status: 'connecting' });
      setRevision((value) => value + 1);
    },
  };
}

export type ChannelActivityFeed = ReturnType<typeof useChannelActivity>;
