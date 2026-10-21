import { useState } from 'react';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { segment, ServiceError, serviceRequest, type Denied } from './client';
export interface Channel {
  channel_id: string;
  record_id: string;
  version: number;
  provider: { slug: string; level: string };
  definition: {
    name: string;
    sources: unknown[];
    schema_version?: number;
    profile?: unknown;
    provider?: {
      target?: { type: 'profile' } | { type: 'existing_agent'; agent_id: string };
    };
    partition?: unknown;
    reply?: unknown;
  };
  endpoints: {
    custom_event_url: string;
    nyxid_callback_url: string;
    telegram_webhook_url?: string;
  };
}
export type ChannelListItem = Omit<Channel, 'channel_id' | 'endpoints'> & {
  readonly channel_id?: string | null;
  readonly endpoints?: Channel['endpoints'] | null;
};
export function hasChannelAddress(channel: ChannelListItem): channel is Channel {
  return (
    typeof channel.channel_id === 'string' &&
    channel.channel_id.length > 0 &&
    channel.endpoints != null &&
    typeof channel.endpoints.custom_event_url === 'string' &&
    typeof channel.endpoints.nyxid_callback_url === 'string'
  );
}
function policy(channel?: Channel): string {
  if (!channel) return '';
  const { definition: d } = channel;
  return JSON.stringify(
    {
      schema_version: d.schema_version,
      name: d.name,
      profile: d.profile,
      provider: { slug: channel.provider.slug, target: d.provider?.target },
      sources: d.sources.map((source) => {
        if (typeof source !== 'object' || source === null) return source;
        return Object.fromEntries(Object.entries(source).filter(([key]) => key !== 'jwks_url'));
      }),
      partition: d.partition,
      reply: d.reply,
    },
    null,
    2,
  );
}
export function ChannelEditor({
  channel,
  onDone,
  denied,
}: {
  readonly channel?: Channel;
  readonly onDone: (channel?: Channel) => void;
  readonly denied: Denied;
}) {
  const [pending, setPending] = useState<{ key: string; body: string; record: string }>();
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  async function send(request: { key: string; body: string; record: string }) {
    setBusy(true);
    setError(undefined);
    try {
      const value = await serviceRequest<ChannelListItem>(
        'event-gateway',
        channel ? `/v1/channels/${segment(channel.channel_id)}` : '/v1/channels',
        {
          method: channel ? 'PUT' : 'POST',
          body: request.body,
          headers: { 'Idempotency-Key': request.key },
        },
      );
      setPending(undefined);
      onDone(hasChannelAddress(value) ? value : undefined);
    } catch (e) {
      if (e instanceof ServiceError && [401, 403].includes(e.status)) denied(e.status);
      if (e instanceof ServiceError && e.status === 400) setPending(undefined);
      setError(
        e instanceof Error
          ? e.message
          : 'The result is unknown. Recover the original request before creating another channel.',
      );
    } finally {
      setBusy(false);
    }
  }
  return (
    <form
      className="service-form"
      onSubmit={(event) => {
        event.preventDefault();
        if (pending) {
          void send(pending);
          return;
        }
        try {
          const fields = new FormData(event.currentTarget);
          const input: unknown = JSON.parse(String(fields.get('policy')));
          if (!input || typeof input !== 'object' || Array.isArray(input))
            throw new Error('Enter a channel policy object.');
          const key = crypto.randomUUID();
          const record = channel?.record_id ?? crypto.randomUUID();
          const body = JSON.stringify({
            ...input,
            ...(channel
              ? { expected_version: channel.version }
              : { record_id: record, credentials: { agent_key: String(fields.get('agent_key')) } }),
          });
          const request = { key, record, body };
          setPending(request);
          void send(request);
        } catch {
          setError('Enter valid JSON using the Event Gateway channel policy schema.');
        }
      }}
    >
      <label>
        Channel configuration JSON
        <textarea
          name="policy"
          defaultValue={policy(channel)}
          rows={12}
          required
          disabled={pending !== undefined}
          maxLength={60000}
        />
      </label>
      <p>
        Supply name, profile, provider, sources, partition and reply using the{' '}
        <a
          className="service-link"
          href="/event-gateway/openapi.json"
          target="_blank"
          rel="noreferrer"
        >
          Event Gateway API schema
        </a>
        .{' '}
        {channel
          ? 'Existing credentials are retained when omitted.'
          : 'The agent key authorizes the configured provider.'}
      </p>
      {!channel ? (
        <label>
          Agent key
          <Input
            name="agent_key"
            type="password"
            autoComplete="off"
            required
            disabled={pending !== undefined}
          />
        </label>
      ) : null}
      {error ? <p role="alert">{error}</p> : null}
      <div className="service-actions">
        <Button type="submit" variant="primary" disabled={busy}>
          {busy
            ? 'Submitting…'
            : pending
              ? 'Retry same request'
              : channel
                ? 'Save channel'
                : 'Create channel'}
        </Button>
        {pending ? (
          <Button
            disabled={busy}
            onClick={() => {
              setBusy(true);
              void serviceRequest<Channel>(
                'event-gateway',
                `/v1/channel-records/${segment(pending.record)}`,
              )
                .then((current) => {
                  // Recovery can return a reservation before provider binding
                  // completes. Only replaying the original create confirms it.
                  setError(
                    channel && current.version > channel.version
                      ? 'The channel has changed. Reload to review its current configuration before making further edits.'
                      : 'The original record was found. Retry the same request to confirm completion.',
                  );
                })
                .catch((e: unknown) => {
                  if (e instanceof ServiceError && [401, 403].includes(e.status)) denied(e.status);
                  setError(
                    e instanceof Error
                      ? e.message
                      : 'Recovery is unavailable. Keep this request for retry.',
                  );
                })
                .finally(() => setBusy(false));
            }}
          >
            Check original channel
          </Button>
        ) : null}
      </div>
    </form>
  );
}
export function DeleteChannel({
  channel,
  onDone,
  denied,
}: {
  readonly channel: Channel;
  readonly onDone: () => void;
  readonly denied: Denied;
}) {
  const [confirm, setConfirm] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  return (
    <div>
      {confirm ? (
        <>
          <p>
            Delete {channel.definition.name}? This disconnects its provider and stops incoming event
            delivery.
          </p>
          <Button
            disabled={busy}
            onClick={() => {
              setBusy(true);
              void serviceRequest(
                'event-gateway',
                `/v1/channels/${segment(channel.channel_id)}?expected_version=${channel.version}`,
                { method: 'DELETE' },
              )
                .then(onDone)
                .catch((e: unknown) => {
                  if (e instanceof ServiceError && [401, 403].includes(e.status)) denied(e.status);
                  setError(
                    e instanceof Error
                      ? e.message
                      : 'Deletion is unconfirmed. Refresh the channel before retrying.',
                  );
                })
                .finally(() => setBusy(false));
            }}
          >
            {busy ? 'Deleting…' : 'Confirm delete'}
          </Button>{' '}
          <Button disabled={busy} onClick={() => setConfirm(false)}>
            Cancel
          </Button>
        </>
      ) : (
        <Button onClick={() => setConfirm(true)}>Delete channel</Button>
      )}
      {error ? <p role="alert">{error}</p> : null}
    </div>
  );
}
