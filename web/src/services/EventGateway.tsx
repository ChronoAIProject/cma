import { useState, type ReactNode } from 'react';
import { Button } from '../ui/Button';
import { routePath } from '../routing/routes';
import { PageHeader, type Crumb } from '../ui/PageHeader';
import { useDocumentTitle } from '../ui/useDocumentTitle';
import {
  can,
  segment,
  ServiceError,
  serviceRequest,
  type Denied,
  type ServiceSession,
} from './client';
import { PageStatus, usePage } from './data';
import {
  ChannelEditor,
  DeleteChannel,
  hasChannelAddress,
  type Channel,
  type ChannelListItem,
} from './ChannelEditor';
import { ChannelEndpoints } from './ChannelEndpoints';
import { ChannelTable } from './ChannelTable';
import { ExistingChannel } from './ExistingChannel';
import './EventGateway.css';

export interface EventGatewaySlots {
  readonly sidebar: ReactNode;
  readonly content: ReactNode;
}
export function EventGateway({
  session,
  denied,
  children,
  onNavigate,
}: {
  readonly session: ServiceSession;
  readonly denied: Denied;
  readonly children?: (slots: EventGatewaySlots) => ReactNode;
  readonly onNavigate?: () => void;
}) {
  const channels = usePage<ChannelListItem>('event-gateway', '/v1/channels', denied);
  const [selected, setSelected] = useState<string>();
  const [visited, setVisited] = useState<readonly string[]>([]);
  const [creating, setCreating] = useState(false);
  const [createStarted, setCreateStarted] = useState(false);
  const [created, setCreated] = useState<Channel>();
  const [deleted, setDeleted] = useState<readonly string[]>([]);
  const rows = channels.value?.items ?? [];
  const items = (
    created && !rows.some((row) => row.record_id === created.record_id) ? [created, ...rows] : rows
  ).filter((row) => !deleted.includes(row.record_id));
  // Nothing is open until somebody chooses a channel: the page opens on the
  // table of every channel, which a collapsed sidebar cannot hide.
  const current = items.find((row) => row.record_id === selected);
  const canCreate = can(session, 'event-gateway', 'channels:create');
  useDocumentTitle(creating ? 'New channel' : (current?.definition.name ?? 'Event Gateway'));
  const retain = () => {
    if (current) setVisited((previous) => [...new Set([...previous, current.record_id])]);
  };
  const select = (id: string) => {
    retain();
    setVisited((previous) => [...new Set([...previous, id])]);
    setSelected(id);
    setCreating(false);
    onNavigate?.();
  };
  const startCreate = () => {
    retain();
    setCreateStarted(true);
    setCreating(true);
    onNavigate?.();
  };
  // A channel's breadcrumb returns to the table; its editor stays mounted.
  const channelsCrumb: Crumb = {
    label: 'Channels',
    href: routePath({ kind: 'event_gateway' }),
    onSelect: () => {
      retain();
      setSelected(undefined);
    },
  };
  const sidebar = (
    <section className="gateway-sidebar" aria-label="Channels">
      <div className="gateway-sidebar__heading">
        <h2>Channels</h2>
        <Button disabled={channels.busy} onClick={channels.refresh} aria-label="Refresh channels">
          Refresh
        </Button>
      </div>
      {canCreate ? <Button onClick={startCreate}>New channel</Button> : null}
      {/* The page's table already says it is loading; the rail keeps the failure. */}
      <PageStatus error={channels.error} retry={channels.refresh} />
      {channels.value && items.length === 0 ? (
        <p className="gateway-sidebar__empty">No channels yet.</p>
      ) : null}
      <ul className="gateway-sidebar__list">
        {items.map((channel) => (
          <li key={channel.record_id}>
            <button
              type="button"
              className="gateway-sidebar__channel"
              aria-current={
                !creating && current?.record_id === channel.record_id ? 'page' : undefined
              }
              onClick={() => select(channel.record_id)}
            >
              <span>{channel.definition.name}</span>
              <small>
                {channel.provider.slug} · {channel.definition.sources.length} sources
              </small>
            </button>
          </li>
        ))}
      </ul>
      {channels.value?.next_cursor ? (
        <Button disabled={channels.busy} onClick={channels.more}>
          Load more channels
        </Button>
      ) : null}
    </section>
  );
  const content = (
    <div className="gateway-content">
      {createStarted ? (
        <section className="gateway-detail" hidden={!creating}>
          <PageHeader
            breadcrumb={[{ label: 'Event Gateway' }, { label: 'New channel' }]}
            title="New channel"
            description="Connect an event source to your agent."
          />
          <ChannelEditor
            denied={denied}
            onDone={(channel) => {
              if (channel) {
                setCreated(channel);
                setSelected(channel.record_id);
              }
              setCreating(false);
              setCreateStarted(false);
              channels.refresh();
            }}
          />
          <Button onClick={() => setCreating(false)}>Cancel</Button>
        </section>
      ) : null}
      {items
        .filter((row) => row.record_id === current?.record_id || visited.includes(row.record_id))
        .map((channel) => (
          <section
            key={channel.record_id}
            hidden={creating || channel.record_id !== current?.record_id}
            className="gateway-detail"
            aria-label={`${channel.definition.name} details`}
          >
            <ChannelDetail
              channel={channel}
              channelsCrumb={channelsCrumb}
              session={session}
              denied={denied}
              onChanged={(updated) => {
                if (updated && updated.record_id === created?.record_id) setCreated(updated);
                channels.refresh();
              }}
              onDeleted={() => {
                setDeleted((previous) => [...previous, channel.record_id]);
                if (created?.record_id === channel.record_id) setCreated(undefined);
                channels.refresh();
              }}
            />
          </section>
        ))}
      {!creating && !current ? (
        <section className="gateway-empty">
          <PageHeader
            breadcrumb={[{ label: 'Event Gateway' }, { label: 'Channels' }]}
            title="Event Gateway"
            description="Choose a channel to view its endpoints, configure event delivery, or send a test."
          />
          <ChannelTable
            channels={items}
            loading={channels.busy && !channels.value}
            unavailable={
              channels.value === undefined && channels.error !== undefined
                ? 'Channels could not be loaded.'
                : undefined
            }
            onSelect={select}
          />
        </section>
      ) : null}
    </div>
  );
  return children ? (
    children({ sidebar, content })
  ) : (
    <div className="gateway-layout">
      {sidebar}
      {content}
    </div>
  );
}

function ChannelDetail({
  channel,
  channelsCrumb,
  session,
  denied,
  onChanged,
  onDeleted,
}: {
  readonly channel: ChannelListItem;
  readonly channelsCrumb: Crumb;
  readonly session: ServiceSession;
  readonly denied: Denied;
  readonly onChanged: (channel?: Channel) => void;
  readonly onDeleted: () => void;
}) {
  return (
    <>
      <PageHeader
        breadcrumb={[{ label: 'Event Gateway' }, channelsCrumb, { label: channel.definition.name }]}
        title={channel.definition.name}
        meta={
          <span className="page-header__chip">
            {channel.provider.slug} · {channel.provider.level} · Version {channel.version}
          </span>
        }
      />
      {hasChannelAddress(channel) ? (
        <AddressedChannel
          channel={channel}
          session={session}
          denied={denied}
          onChanged={onChanged}
          onDeleted={onDeleted}
        />
      ) : (
        <ExistingChannel channel={channel} denied={denied}>
          {(resolved) => (
            <AddressedChannel
              channel={resolved}
              session={session}
              denied={denied}
              onChanged={onChanged}
              onDeleted={onDeleted}
            />
          )}
        </ExistingChannel>
      )}
    </>
  );
}
function AddressedChannel({
  channel,
  session,
  denied,
  onChanged,
  onDeleted,
}: {
  readonly channel: Channel;
  readonly session: ServiceSession;
  readonly denied: Denied;
  readonly onChanged: (channel?: Channel) => void;
  readonly onDeleted: () => void;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState<Channel>();
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<string>();
  async function test() {
    setBusy(true);
    setResult(undefined);
    try {
      const value = await serviceRequest<{ first_text: string }>(
        'event-gateway',
        `/v1/channels/${segment(channel.channel_id)}/test`,
        { method: 'POST', signal: AbortSignal.timeout(125_000) },
      );
      setResult(value.first_text);
    } catch (error) {
      if (error instanceof ServiceError && [401, 403].includes(error.status)) denied(error.status);
      setResult(
        error instanceof Error && error.name !== 'TimeoutError'
          ? error.message
          : 'Test result is unknown. Check channel activity before sending another test.',
      );
    } finally {
      setBusy(false);
    }
  }
  return (
    <>
      <ChannelEndpoints endpoints={channel.endpoints} />
      <div className="service-actions">
        {can(session, 'event-gateway', 'channels:update') ? (
          <Button
            onClick={() => {
              setDraft((current) => current ?? channel);
              setEditing((value) => !value);
            }}
          >
            {editing ? 'Close editor' : 'Edit channel'}
          </Button>
        ) : null}
        {can(session, 'event-gateway', 'channels:test') ? (
          <Button disabled={busy} onClick={() => void test()}>
            {busy ? 'Sending test…' : 'Send test event'}
          </Button>
        ) : null}
      </div>
      {can(session, 'event-gateway', 'channels:update') && draft ? (
        <div hidden={!editing}>
          <ChannelEditor
            channel={draft}
            denied={denied}
            onDone={(updated) => {
              setEditing(false);
              setDraft(undefined);
              onChanged(updated);
            }}
          />
        </div>
      ) : null}
      {result ? (
        <section className="service-panel" role="status">
          <h2>Test result</h2>
          <p>{result}</p>
        </section>
      ) : null}
      <details className="gateway-detail__section">
        <summary>Channel configuration</summary>
        <pre>{JSON.stringify(channel.definition, null, 2)}</pre>
      </details>
      {can(session, 'event-gateway', 'channels:delete') ? (
        <section className="gateway-detail__section">
          <h2>Delete channel</h2>
          <DeleteChannel channel={channel} denied={denied} onDone={onDeleted} />
        </section>
      ) : null}
    </>
  );
}
