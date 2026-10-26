import { useId, useState, type ReactNode } from 'react';
import { Button } from '../ui/Button';
import { routePath } from '../routing/routes';
import { PageHeader, type Crumb } from '../ui/PageHeader';
import { useDocumentTitle } from '../ui/useDocumentTitle';
import { can, type Denied, type ServiceSession } from './client';
import { PageStatus, usePage } from './data';
import {
  ChannelEditor,
  DeleteChannel,
  hasChannelAddress,
  type Channel,
  type ChannelListItem,
} from './ChannelEditor';
import { ChannelMap } from './ChannelMap';
import { NodeInspector } from './NodeInspector';
import { ChannelNodeTests } from './ChannelNodeTests';
import { channelNodes, selectionKey, type ChannelSelection } from './channelTopology';
import { ChannelTable } from './ChannelTable';
import { ExistingChannel } from './ExistingChannel';
import { RelayCapabilitiesProvider } from './RelayCapabilities';
import './EventGateway.css';

export interface EventGatewaySlots {
  readonly sidebar: ReactNode;
  readonly content: ReactNode;
}
interface EventGatewayProps {
  readonly session: ServiceSession;
  readonly denied: Denied;
  readonly children?: (slots: EventGatewaySlots) => ReactNode;
  readonly onNavigate?: () => void;
}
export function EventGateway(props: EventGatewayProps) {
  const identity = JSON.stringify([props.session.subject, [...props.session.permissions].sort()]);
  return (
    <RelayCapabilitiesProvider key={identity} denied={props.denied}>
      <EventGatewayBody {...props} />
    </RelayCapabilitiesProvider>
  );
}
function EventGatewayBody({ session, denied, children, onNavigate }: EventGatewayProps) {
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
            ownerSubject={session.subject}
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
  const inspectorId = useId();
  const [selection, setSelection] = useState<ChannelSelection>({ kind: 'channel' });
  const [confirmed, setConfirmed] = useState<Channel>();
  const current = confirmed && confirmed.version >= channel.version ? confirmed : channel;
  const nodes = channelNodes(current);
  const editable = can(session, 'event-gateway', 'channels:update');
  const newProducer =
    editable && selection.kind === 'producer' && selection.index === nodes.producers.length;
  const exists =
    selection.kind === 'channel' ||
    newProducer ||
    [...nodes.producers, ...nodes.consumers].some((node) => node.key === selectionKey(selection));
  const selected: ChannelSelection = exists ? selection : { kind: 'channel' };
  return (
    <div className="channel-workspace">
      <ChannelMap
        channel={current}
        selection={selected}
        onSelect={(next) => {
          setSelection(next);
          requestAnimationFrame(() => {
            const inspector = document.getElementById(inspectorId);
            if (
              !inspector ||
              inspector.getClientRects().length === 0 ||
              (inspector.parentElement?.clientWidth ?? 0) >= 1080
            )
              return;
            inspector.scrollIntoView?.({
              block: 'start',
              behavior: window.matchMedia('(prefers-reduced-motion: reduce)').matches
                ? 'instant'
                : 'smooth',
            });
          });
        }}
        inspectorId={inspectorId}
        canAddProducer={editable}
      />
      <NodeInspector channel={current} selection={selected} id={inspectorId}>
        {editable ? (
          <ChannelEditor
            channel={current}
            section={selected}
            denied={denied}
            onDone={(updated) => {
              if (updated) setConfirmed(updated);
              onChanged(updated);
            }}
          />
        ) : (
          <p className="channel-field-hint">You have read-only access to this configuration.</p>
        )}
        {can(session, 'event-gateway', 'channels:test') ? (
          <div hidden={newProducer}>
            <ChannelNodeTests
              channelId={current.channel_id}
              channelVersion={current.version}
              selection={selected}
              denied={denied}
            />
          </div>
        ) : null}
        <div hidden={selected.kind !== 'channel'}>
          {can(session, 'event-gateway', 'channels:delete') ? (
            <section className="channel-inspector__danger">
              <h3>Delete channel</h3>
              <DeleteChannel channel={current} denied={denied} onDone={onDeleted} />
            </section>
          ) : null}
        </div>
      </NodeInspector>
    </div>
  );
}
