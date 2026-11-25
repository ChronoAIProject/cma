import { Button } from '../ui/Button';
import { PageHeader, type Crumb } from '../ui/PageHeader';
import { AddressedChannel } from './AddressedChannel';
import { ChannelLifecycleStatus } from './ChannelLifecycleStatus';
import { ExistingChannel } from './ExistingChannel';
import { channelEvidenceState, channelIsWritable, type ChannelEvidence } from './channelEvidence';
import { hasChannelAddress, type Channel, type ChannelListItem } from './channelTypes';
import type { Denied, ServiceSession } from './client';

export function ChannelDetail({
  evidence,
  stale,
  active,
  channelsCrumb,
  session,
  denied,
  onRead,
  onUnavailable,
  refresh,
  onChanged,
  onDeleted,
}: {
  readonly evidence: ChannelEvidence;
  readonly stale: boolean;
  readonly channelsCrumb: Crumb;
  readonly active: boolean;
  readonly session: ServiceSession;
  readonly denied: Denied;
  readonly onRead: (channel: ChannelListItem) => void;
  readonly onUnavailable: (record: string) => void;
  readonly refresh: () => void;
  readonly onChanged: (channel?: Channel) => void;
  readonly onDeleted: () => void;
}) {
  const { channel, address } = evidence;
  const readIsStale = stale || Boolean(evidence.stale);
  const deletable =
    !readIsStale &&
    !evidence.unconfirmed &&
    hasChannelAddress(channel) &&
    Number.isSafeInteger(channel.version) &&
    channel.version >= 0;
  return (
    <>
      <PageHeader
        wrapIdentity
        breadcrumb={[{ label: 'Event Gateway' }, channelsCrumb, { label: channel.definition.name }]}
        title={channel.definition.name}
        meta={
          <span className="page-header__chip">
            {channel.provider.slug} · {channel.provider.level} · Version {channel.version}
          </span>
        }
      />
      <ChannelLifecycleStatus
        state={channelEvidenceState(evidence)}
        addressAvailable={hasChannelAddress(channel)}
        stale={readIsStale}
      />
      <Button onClick={refresh}>Refresh channel status</Button>
      <ExistingChannel
        channel={channel}
        originalAddress={address?.channel_id}
        denied={denied}
        onRead={onRead}
        onUnavailable={onUnavailable}
      />
      {address ? (
        <AddressedChannel
          channel={{ ...channel, ...address }}
          active={active}
          writable={channelIsWritable(evidence, readIsStale)}
          deletable={deletable}
          session={session}
          denied={denied}
          onChanged={onChanged}
          onDeleted={onDeleted}
        />
      ) : null}
    </>
  );
}
