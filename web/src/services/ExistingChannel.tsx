import { useEffect, useRef, useState, type ReactNode } from 'react';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { hasChannelAddress, type Channel, type ChannelListItem } from './ChannelEditor';
import { segment, ServiceError, serviceRequest } from './client';

/** Legacy records retain their original opaque address; never invent a replacement ID. */
export function ExistingChannel({
  channel,
  denied,
  children,
}: {
  readonly channel: ChannelListItem;
  readonly denied: () => void;
  readonly children: (channel: Channel) => ReactNode;
}) {
  const [resolved, setResolved] = useState<Channel>();
  const address = useRef<string | undefined>(undefined);
  const [busy, setBusy] = useState(true);
  const [error, setError] = useState<string>();
  useEffect(() => {
    const controller = new AbortController();
    void serviceRequest<ChannelListItem>(
      'event-gateway',
      (channel.channel_id ?? address.current)
        ? `/v1/channels/${segment((channel.channel_id ?? address.current)!)}`
        : `/v1/channel-records/${segment(channel.record_id)}`,
      { signal: AbortSignal.any([controller.signal, AbortSignal.timeout(15000)]) },
    )
      .then((value) => {
        if (
          !controller.signal.aborted &&
          value.record_id === channel.record_id &&
          hasChannelAddress(value)
        ) {
          address.current = value.channel_id;
          setResolved(value);
        }
      })
      .catch((reason: unknown) => {
        if (
          !controller.signal.aborted &&
          reason instanceof ServiceError &&
          [401, 403].includes(reason.status)
        )
          denied();
      })
      .finally(() => {
        if (!controller.signal.aborted) setBusy(false);
      });
    return () => controller.abort();
  }, [channel.record_id, channel.channel_id, channel.version, denied]);
  if (resolved && resolved.version >= channel.version) return children(resolved);
  return (
    <>
      <p>This existing channel keeps its original configuration and address.</p>
      {busy ? (
        <p role="status">Loading channel endpoints…</p>
      ) : (
        <form
          className="service-form"
          onSubmit={(event) => {
            event.preventDefault();
            const id = String(new FormData(event.currentTarget).get('channel_id')).trim();
            if (!id) return;
            setBusy(true);
            setError(undefined);
            void serviceRequest<ChannelListItem>('event-gateway', `/v1/channels/${segment(id)}`)
              .then((value) => {
                if (
                  value.record_id !== channel.record_id ||
                  !hasChannelAddress(value) ||
                  value.channel_id !== id
                )
                  throw new Error(
                    'This channel ID belongs to another channel. Use the original ID for this record.',
                  );
                address.current = value.channel_id;
                setResolved(value);
              })
              .catch((reason: unknown) => {
                if (reason instanceof ServiceError && [401, 403].includes(reason.status)) denied();
                setError(
                  reason instanceof Error ? reason.message : 'The channel could not be opened.',
                );
              })
              .finally(() => setBusy(false));
          }}
        >
          <label>
            Original channel ID
            <Input name="channel_id" required autoComplete="off" />
          </label>
          <p>Use the channel ID returned when this channel was created to manage it here.</p>
          <Button type="submit">Open existing channel</Button>
        </form>
      )}
      {error ? <p role="alert">{error}</p> : null}
      <details className="gateway-detail__section">
        <summary>Channel configuration</summary>
        <pre>{JSON.stringify(channel.definition, null, 2)}</pre>
      </details>
      <a
        className="service-link"
        href={`/event-gateway/legacy-console/channels/${segment(channel.record_id)}`}
        target="_blank"
        rel="noreferrer"
      >
        Open existing channel console
      </a>
    </>
  );
}
