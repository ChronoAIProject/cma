import { useId, useState, type ReactNode } from 'react';
import { TreeRow } from '../shell/TreeRow';
import { keyboardNavigation } from '../shell/treePresentation';
import type { SidebarNewAgent } from '../shell/Sidebar';
import { IconBranchOutline16, IconRefreshOutline16 } from '../ui/icons';
import '../shell/Tree.css';
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
import { ChannelOutboundMessage } from './ChannelOutboundMessage';
import { channelNodes, selectionKey, type ChannelSelection } from './channelTopology';
import { ChannelActivityView } from './ChannelActivity';
import { ChannelActivityPreview } from './ChannelActivityPreview';
import { useChannelActivity } from './useChannelActivity';
import { GatewayWelcome } from './GatewayWelcome';
import { GatewayRecordMeta } from './GatewayRecordMeta';
import { ExistingChannel } from './ExistingChannel';
import { RelayCapabilitiesProvider } from './RelayCapabilities';
import './EventGateway.css';

export interface EventGatewaySlots {
  readonly sidebar: ReactNode;
  readonly createAction?: SidebarNewAgent;
  readonly content: ReactNode;
}
interface EventGatewayProps {
  readonly session: ServiceSession;
  readonly denied: Denied;
  readonly children?: (slots: EventGatewaySlots) => ReactNode;
  readonly onNavigate?: () => void;
  readonly active?: boolean;
}
export function EventGateway(props: EventGatewayProps) {
  const identity = JSON.stringify([props.session.subject, [...props.session.permissions].sort()]);
  return (
    <RelayCapabilitiesProvider key={identity} denied={props.denied}>
      <EventGatewayBody {...props} />
    </RelayCapabilitiesProvider>
  );
}
function EventGatewayBody({
  session,
  denied,
  children,
  onNavigate,
  active = true,
}: EventGatewayProps) {
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
  // welcome surface until a channel is selected.
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
  // A channel's breadcrumb returns home; its editor stays mounted.
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
        <button
          type="button"
          className="navigation-filters__toggle"
          disabled={channels.busy}
          onClick={channels.refresh}
          aria-label="Refresh channels"
          title="Refresh channels"
        >
          <IconRefreshOutline16 />
        </button>
      </div>
      {/* The welcome surface shows loading; the rail retains failures. */}
      <PageStatus error={channels.error} retry={channels.refresh} />
      {channels.value && items.length === 0 ? (
        <p className="gateway-sidebar__empty">No channels yet.</p>
      ) : null}
      <div
        className="agent-navigation"
        role="tree"
        aria-label="Channels"
        onKeyDown={keyboardNavigation}
      >
        {items.map((channel) => (
          <TreeRow
            key={channel.record_id}
            nodeId={`channel:${channel.record_id}`}
            level={1}
            layout="line"
            label={channel.definition.name}
            icon={null}
            selected={!creating && current?.record_id === channel.record_id}
            onNavigate={() => {}}
            onActivate={() => select(channel.record_id)}
            meta={
              <GatewayRecordMeta
                compact
                createdAt={
                  channel.created_at ??
                  (typeof channel.definition.created_at === 'string'
                    ? channel.definition.created_at
                    : undefined)
                }
                owner={channel.owner ?? channel.definition.owner}
              />
            }
          />
        ))}
      </div>
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
              active={active && !creating && channel.record_id === current?.record_id}
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
        <GatewayWelcome
          kind="channel"
          onCreate={canCreate ? startCreate : undefined}
          loading={channels.busy && !channels.value}
        />
      ) : null}
    </div>
  );
  return children ? (
    children({
      sidebar,
      content,
      createAction: canCreate
        ? { label: 'New channel', icon: <IconBranchOutline16 />, onCreate: startCreate }
        : undefined,
    })
  ) : (
    <div className="gateway-layout">
      <div>
        {canCreate ? (
          <button type="button" className="sidebar__new" onClick={startCreate}>
            <IconBranchOutline16 />
            <span>New channel</span>
          </button>
        ) : null}
        {sidebar}
      </div>
      {content}
    </div>
  );
}

function ChannelDetail({
  channel,
  active,
  channelsCrumb,
  session,
  denied,
  onChanged,
  onDeleted,
}: {
  readonly channel: ChannelListItem;
  readonly channelsCrumb: Crumb;
  readonly active: boolean;
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
          active={active}
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
              active={active}
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
  active,
  session,
  denied,
  onChanged,
  onDeleted,
}: {
  readonly channel: Channel;
  readonly active: boolean;
  readonly session: ServiceSession;
  readonly denied: Denied;
  readonly onChanged: (channel?: Channel) => void;
  readonly onDeleted: () => void;
}) {
  const inspectorId = useId();
  const [inspecting, setInspecting] = useState(false);
  const [inspectorSection, setInspectorSection] = useState<'overview' | 'activity'>('overview');
  const [selection, setSelection] = useState<ChannelSelection>({ kind: 'channel' });
  const [editorSelection, setEditorSelection] = useState<
    Exclude<ChannelSelection, { kind: 'toolsets' }>
  >({ kind: 'channel' });
  const [confirmed, setConfirmed] = useState<Channel>();
  const current = confirmed && confirmed.version >= channel.version ? confirmed : channel;
  const activity = useChannelActivity(current.channel_id, session.subject, active);
  const nodes = channelNodes(current);
  const editable = can(session, 'event-gateway', 'channels:update');
  const newProducer =
    editable && selection.kind === 'producer' && selection.index === nodes.producers.length;
  const exists =
    selection.kind === 'channel' ||
    selection.kind === 'toolsets' ||
    newProducer ||
    [...nodes.producers, ...nodes.consumers].some((node) => node.key === selectionKey(selection));
  const selected: ChannelSelection = exists ? selection : { kind: 'channel' };
  return (
    <div className="channel-workspace">
      <ChannelMap
        channel={current}
        toolsets={nodes.toolsets}
        selection={selected}
        activityPreview={<ChannelActivityPreview feed={activity} />}
        onActivity={() => {
          setSelection({ kind: 'channel' });
          setEditorSelection({ kind: 'channel' });
          setInspectorSection('activity');
          setInspecting(true);
        }}
        onSelect={(next) => {
          setInspectorSection('overview');
          setSelection(next);
          if (next.kind !== 'toolsets') setEditorSelection(next);
          setInspecting(true);
        }}
        inspectorId={inspectorId}
        canAddProducer={editable}
      />
      <NodeInspector
        channel={current}
        selection={selected}
        id={inspectorId}
        open={active && inspecting}
        initialSection={inspectorSection}
        onClose={() => setInspecting(false)}
        send={
          editable ? (
            <ChannelOutboundMessage
              owner={session.subject}
              channel={current}
              selection={selected}
              denied={denied}
            />
          ) : undefined
        }
        tests={
          can(session, 'event-gateway', 'channels:test') ? (
            <div hidden={newProducer}>
              <ChannelNodeTests
                channelId={current.channel_id}
                channelVersion={current.version}
                selection={selected}
                denied={denied}
              />
            </div>
          ) : undefined
        }
        activity={selected.kind === 'channel' ? <ChannelActivityView feed={activity} /> : undefined}
        danger={
          selected.kind === 'channel' && can(session, 'event-gateway', 'channels:delete') ? (
            <DeleteChannel channel={current} denied={denied} onDone={onDeleted} />
          ) : undefined
        }
      >
        {editable ? (
          <ChannelEditor
            channel={current}
            section={selected.kind === 'toolsets' ? editorSelection : selected}
            denied={denied}
            onDone={(updated) => {
              if (updated) setConfirmed(updated);
              onChanged(updated);
            }}
          />
        ) : (
          <p className="channel-field-hint">You have read-only access to this configuration.</p>
        )}
      </NodeInspector>
    </div>
  );
}
