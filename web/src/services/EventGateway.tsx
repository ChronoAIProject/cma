import { useState, type ReactNode } from 'react';
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
import { ChannelEditor, type Channel, type ChannelListItem } from './ChannelEditor';
import { GatewayWelcome } from './GatewayWelcome';
import { GatewayRecordMeta } from './GatewayRecordMeta';
import { ChannelDetail } from './ChannelDetail';
import { ChannelLifecycleStatus } from './ChannelLifecycleStatus';
import { channelEvidenceState } from './channelEvidence';
import { hasChannelAddress } from './channelTypes';
import { useChannelRecords } from './useChannelRecords';
import { duplicateChannelLabels } from './channelLabels';
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
  const evidence = useChannelRecords(channels.value, created);
  const refresh = () => {
    evidence.beginRefresh();
    channels.refresh();
  };
  const stale = channels.busy || Boolean(channels.error);
  const rows =
    channels.value?.items ??
    (channels.error ? [...evidence.records.values()].map((value) => value.channel) : []);
  const items = (
    created && !rows.some((row) => row.record_id === created.record_id) ? [created, ...rows] : rows
  )
    .filter((row) => !deleted.includes(row.record_id))
    .map((row) => evidence.records.get(row.record_id)!.channel);
  const labels = duplicateChannelLabels(items);
  // Nothing is open until somebody chooses a channel: the page opens on the
  // welcome surface until a channel is selected.
  const workspaces = [...new Set([...visited, ...(selected ? [selected] : [])])]
    .filter((id) => !deleted.includes(id))
    .flatMap((id) => {
      const record = evidence.records.get(id);
      return record ? [record.channel] : [];
    });
  const current = workspaces.find((row) => row.record_id === selected);
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
  const exitCreate = () => setCreating(false);
  const showChannels = () => {
    retain();
    setSelected(undefined);
  };
  const homeHref = routePath({ kind: 'event_gateway' });
  // A channel's breadcrumb returns home; its editor stays mounted.
  const channelsCrumb: Crumb = {
    label: 'Channels',
    href: homeHref,
    onSelect: showChannels,
  };
  const sidebar = (
    <section className="gateway-sidebar" aria-label="Channels">
      <div className="gateway-sidebar__heading">
        <button
          type="button"
          className="navigation-filters__toggle"
          disabled={channels.busy}
          onClick={refresh}
          aria-label="Refresh channels"
          title="Refresh channels"
        >
          <IconRefreshOutline16 />
        </button>
      </div>
      {/* The welcome surface shows loading; the rail retains failures. */}
      <PageStatus error={channels.error} retry={refresh} />
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
            label={labels.get(channel.record_id) ?? channel.definition.name}
            icon={null}
            selected={!creating && current?.record_id === channel.record_id}
            onNavigate={() => {}}
            onActivate={() => select(channel.record_id)}
            meta={
              <>
                <ChannelLifecycleStatus
                  compact
                  state={channelEvidenceState(evidence.records.get(channel.record_id)!)}
                  addressAvailable={hasChannelAddress(channel)}
                  stale={stale || Boolean(evidence.records.get(channel.record_id)!.stale)}
                />
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
              </>
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
  const createActive = active && creating;
  const content = (
    <div className="gateway-content">
      {createStarted ? (
        <section className="gateway-detail gateway-detail--create" hidden={!createActive}>
          <PageHeader
            wrapIdentity
            breadcrumb={[
              {
                label: 'Event Gateway',
                href: homeHref,
                onSelect: () => {
                  showChannels();
                  exitCreate();
                },
              },
              { label: 'New channel' },
            ]}
            title="New channel"
            description="Connect an event source to your agent."
          />
          <ChannelEditor
            active={createActive}
            ownerSubject={session.subject}
            denied={denied}
            onDone={(channel) => {
              if (channel) {
                setCreated(channel);
                setSelected(channel.record_id);
              }
              setCreating(false);
              setCreateStarted(false);
              refresh();
            }}
          />
          <Button onClick={exitCreate}>Cancel</Button>
        </section>
      ) : null}
      {workspaces.map((channel) => (
        <section
          key={channel.record_id}
          hidden={creating || channel.record_id !== current?.record_id}
          className="gateway-detail"
          aria-label={`${channel.definition.name} details`}
        >
          <ChannelDetail
            evidence={evidence.records.get(channel.record_id)!}
            stale={stale}
            onRead={evidence.observe}
            onUnavailable={evidence.unavailable}
            refresh={refresh}
            active={active && !creating && channel.record_id === current?.record_id}
            channelsCrumb={channelsCrumb}
            session={session}
            denied={denied}
            onChanged={(updated) => {
              if (updated) evidence.observe(updated);
              if (updated && updated.record_id === created?.record_id) setCreated(updated);
              refresh();
            }}
            onDeleted={() => {
              setDeleted((previous) => [...previous, channel.record_id]);
              if (created?.record_id === channel.record_id) setCreated(undefined);
              refresh();
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
