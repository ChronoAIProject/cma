import { useState } from 'react';
import { Button } from '../ui/Button';
import { can, segment, ServiceError, serviceRequest, type ServiceSession } from './client';
import { PageStatus, usePage } from './data';
import {
  ChannelEditor,
  DeleteChannel,
  hasChannelAddress,
  type ChannelListItem,
} from './ChannelEditor';
export function EventGateway({
  session,
  denied,
}: {
  readonly session: ServiceSession;
  readonly denied: () => void;
}) {
  const channels = usePage<ChannelListItem>('event-gateway', '/v1/channels', denied);
  const [busy, setBusy] = useState<string>();
  const [creating, setCreating] = useState(false);
  const [result, setResult] = useState<string>();
  async function test(id: string) {
    setBusy(id);
    setResult(undefined);
    try {
      const value = await serviceRequest<{ first_text: string }>(
        'event-gateway',
        `/v1/channels/${segment(id)}/test`,
        { method: 'POST' },
      );
      setResult(value.first_text);
    } catch (e) {
      if (e instanceof ServiceError && [401, 403].includes(e.status)) denied();
      setResult(
        e instanceof Error
          ? e.message
          : 'Test result is unknown. Check channel activity before sending another test.',
      );
    } finally {
      setBusy(undefined);
    }
  }
  return (
    <>
      {can(session, 'event-gateway', 'channels:create') ? (
        <section className="service-panel">
          <Button onClick={() => setCreating(true)}>New channel</Button>
          {creating ? (
            <ChannelEditor
              denied={denied}
              onDone={() => {
                setCreating(false);
                channels.refresh();
              }}
            />
          ) : null}
        </section>
      ) : null}
      <div className="service-actions">
        <h2>Your channels</h2>
        <Button disabled={channels.busy} onClick={channels.refresh}>
          Refresh channels
        </Button>
      </div>
      <PageStatus busy={channels.busy} error={channels.error} retry={channels.refresh} />
      {channels.value?.items.length === 0 ? (
        <p>No channels yet. Channels created through Event Gateway will appear here.</p>
      ) : null}
      <ul className="service-list">
        {channels.value?.items.map((channel) => (
          <li key={channel.record_id}>
            <h3>{channel.definition.name}</h3>
            <p>
              {channel.provider.slug} · Version {channel.version} ·{' '}
              {channel.definition.sources.length} sources
            </p>
            <details>
              <summary>Channel details</summary>
              {hasChannelAddress(channel) ? (
                <>
                  <p>Event endpoint</p>
                  <code>{channel.endpoints.custom_event_url}</code>
                  <p>Callback endpoint</p>
                  <code>{channel.endpoints.nyxid_callback_url}</code>
                </>
              ) : (
                <p>
                  Open this existing channel in the Event Gateway console to view its endpoints and
                  manage it.
                </p>
              )}
              <pre>{JSON.stringify(channel.definition, null, 2)}</pre>
            </details>
            {!hasChannelAddress(channel) ? (
              <a
                className="service-link"
                href={`https://bot-gateway.chrono-ai.fun/console/channels/${segment(channel.record_id)}`}
                target="_blank"
                rel="noreferrer"
              >
                Manage existing channel
              </a>
            ) : null}
            {hasChannelAddress(channel) && can(session, 'event-gateway', 'channels:update') ? (
              <details>
                <summary>Edit channel</summary>
                <ChannelEditor
                  key={channel.version}
                  channel={channel}
                  denied={denied}
                  onDone={channels.refresh}
                />
              </details>
            ) : null}
            {hasChannelAddress(channel) && can(session, 'event-gateway', 'channels:delete') ? (
              <DeleteChannel channel={channel} denied={denied} onDone={channels.refresh} />
            ) : null}
            {hasChannelAddress(channel) && can(session, 'event-gateway', 'channels:test') ? (
              <Button disabled={busy !== undefined} onClick={() => void test(channel.channel_id)}>
                {busy === channel.channel_id ? 'Sending test…' : 'Send test event'}
              </Button>
            ) : null}
          </li>
        ))}
      </ul>
      {channels.value?.next_cursor ? (
        <Button disabled={channels.busy} onClick={channels.more}>
          Load more channels
        </Button>
      ) : null}
      {result ? (
        <section className="service-panel" role="status">
          <h3>Test result</h3>
          <p>{result}</p>
        </section>
      ) : null}
    </>
  );
}
