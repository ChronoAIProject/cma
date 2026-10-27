import { useId, useMemo, type ReactNode } from 'react';
import { Button } from '../ui/Button';
import type { Channel } from './ChannelEditor';
import { DeliveryFlow } from './DeliveryFlow';
import {
  channelNodes,
  selectionKey,
  type ChannelNode,
  type ChannelSelection,
} from './channelTopology';
import './ChannelMap.css';

export function ChannelMap({
  channel,
  selection,
  onSelect,
  onActivity,
  activityPreview,
  toolsets,
  inspectorId,
  canAddProducer = false,
}: {
  readonly channel: Channel;
  readonly selection: ChannelSelection;
  readonly onSelect: (selection: ChannelSelection) => void;
  readonly inspectorId: string;
  readonly onActivity?: () => void;
  readonly activityPreview?: ReactNode;
  readonly toolsets?: ChannelNode;
  readonly canAddProducer?: boolean;
}) {
  const heading = useId();
  const groups = useMemo(() => {
    const { producers, consumers } = channelNodes(channel);
    const center: ChannelNode = {
      key: 'channel',
      selection: { kind: 'channel' },
      title: channel.definition.name,
      subtitle: 'Route events to agents',
      detail: `Version ${channel.version}`,
    };
    return [producers, [center], consumers];
  }, [channel]);
  return (
    <section className="channel-map" aria-labelledby={heading}>
      <div className="channel-map__heading">
        <div>
          <h2 id={heading}>Delivery map</h2>
          <p>Select a node to configure its part of the journey.</p>
        </div>
      </div>
      {channel.native_discord_activation ? (
        <p className="channel-field-hint" role="status" aria-label="Native Discord activation">
          {channel.native_discord_activation.detail}
        </p>
      ) : null}
      <DeliveryFlow
        groups={groups}
        toolsets={toolsets}
        overlay={
          <>
            {activityPreview}
            <div className="channel-map__actions">
              {onActivity ? <Button onClick={onActivity}>Activity</Button> : null}
              {canAddProducer && groups[0]!.length < 4 ? (
                <Button onClick={() => onSelect({ kind: 'producer', index: groups[0]!.length })}>
                  Add producer
                </Button>
              ) : null}
            </div>
          </>
        }
        selected={selectionKey(selection)}
        onSelect={onSelect}
        inspectorId={inspectorId}
      />
      <p className="channel-map__legend">
        Arrows show routing direction · Activity shows recorded events
      </p>
    </section>
  );
}
