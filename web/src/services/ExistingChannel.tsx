import { useCallback, useEffect, useRef, useState } from 'react';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { hasChannelAddress, type ChannelListItem } from './channelTypes';
import { channelResponseMatches } from './channelEvidence';
import { segment, ServiceError, serviceRequest, type Denied } from './client';

/** Legacy records retain their original opaque address; never invent a replacement ID. */
export function ExistingChannel({
  channel,
  originalAddress,
  denied,
  onRead,
  onUnavailable,
}: {
  readonly channel: ChannelListItem;
  readonly originalAddress?: string;
  readonly denied: Denied;
  readonly onRead: (channel: ChannelListItem) => void;
  readonly onUnavailable: (record: string) => void;
}) {
  const active = useRef<AbortController | undefined>(undefined);
  const [busy, setBusy] = useState(true);
  const [error, setError] = useState<string>();
  const [observed, setObserved] = useState(channel);
  if (channel !== observed) {
    setObserved(channel);
    setBusy(!hasChannelAddress(channel));
    setError(undefined);
  }
  const read = useCallback(
    (id?: string) => {
      active.current?.abort();
      const controller = new AbortController();
      active.current = controller;
      return serviceRequest<ChannelListItem>(
        'event-gateway',
        id ? `/v1/channels/${segment(id)}` : `/v1/channel-records/${segment(channel.record_id)}`,
        { signal: AbortSignal.any([controller.signal, AbortSignal.timeout(15000)]) },
      )
        .then((value) => {
          if (controller.signal.aborted) return;
          if (
            !channelResponseMatches(channel, value) ||
            !hasChannelAddress(value) ||
            (id && value.channel_id !== id) ||
            (originalAddress && value.channel_id !== originalAddress)
          )
            throw new Error(
              'This channel ID belongs to another channel or its version could not be confirmed. Use the original ID for this record and refresh.',
            );
          onRead(value);
        })
        .catch((reason: unknown) => {
          if (controller.signal.aborted) return;
          if (reason instanceof ServiceError && [401, 403].includes(reason.status))
            denied(reason.status);
          onUnavailable(channel.record_id);
          setError(reason instanceof Error ? reason.message : 'The channel could not be opened.');
        })
        .finally(() => {
          if (!controller.signal.aborted) setBusy(false);
        });
    },
    [channel, originalAddress, denied, onRead, onUnavailable],
  );
  useEffect(() => {
    if (!hasChannelAddress(channel)) void read(channel.channel_id ?? originalAddress);
    return () => active.current?.abort();
  }, [channel, originalAddress, read]);
  if (hasChannelAddress(channel)) return null;
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
            if (id && !busy) {
              setBusy(true);
              setError(undefined);
              void read(id);
            }
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
