import { useEffect, useState } from 'react';
import { segment } from './client';

import { parseActivitySnapshot, type ActivityPage } from './channelActivitySnapshot';

export type {
  ActivityConsumer,
  ActivityDelivery,
  ActivityPage,
  ChannelActivityEvent,
} from './channelActivitySnapshot';

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
        const page = parseActivitySnapshot(JSON.parse(event.data));
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
