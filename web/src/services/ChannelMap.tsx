import { useId } from 'react';
import { Button } from '../ui/Button';
import { ChannelNodeIcon } from './ChannelNodeIcon';
import type { Channel } from './ChannelEditor';
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
  inspectorId,
  canAddProducer = false,
}: {
  readonly channel: Channel;
  readonly selection: ChannelSelection;
  readonly onSelect: (selection: ChannelSelection) => void;
  readonly inspectorId: string;
  readonly canAddProducer?: boolean;
}) {
  const heading = useId();
  const { producers, consumers } = channelNodes(channel);
  const selected = selectionKey(selection);
  const node = (value: ChannelNode) => (
    <li key={value.key}>
      <button
        type="button"
        className="channel-map__node"
        aria-label={`${value.selection.kind === 'producer' ? 'Producer' : value.selection.kind === 'consumer' ? 'Consumer' : 'Channel'}: ${value.title} (${value.subtitle})`}
        aria-pressed={selected === value.key}
        aria-controls={inspectorId}
        onClick={() => onSelect(value.selection)}
        data-kind={value.selection.kind}
      >
        <span className="channel-map__icon" aria-hidden="true" data-identity={value.identity}>
          <ChannelNodeIcon node={value} />
        </span>
        <span className="channel-map__text">
          <strong title={value.title}>{value.title}</strong>
          <span title={value.subtitle}>{value.subtitle}</span>
          <small title={value.detail} data-pending={value.pending || undefined}>
            {value.detail}
          </small>
        </span>
      </button>
    </li>
  );
  return (
    <section className="channel-map" aria-labelledby={heading}>
      <div className="channel-map__heading">
        <h2 id={heading}>Delivery map</h2>
        <p>Select a node to configure its part of the journey.</p>
      </div>
      <div className="channel-map__stages">
        <section
          className="channel-map__stage channel-map__stage--producers"
          aria-label="Producers"
        >
          <h3>
            Producers <span>{producers.length}</span>
          </h3>
          {producers.length > 0 ? (
            <ul>{producers.map(node)}</ul>
          ) : (
            <p className="channel-map__empty">No producers configured</p>
          )}
          {canAddProducer && producers.length < 4 ? (
            <Button
              placement="channel-map__add"
              onClick={() => onSelect({ kind: 'producer', index: producers.length })}
            >
              Add producer
            </Button>
          ) : null}
        </section>
        <section className="channel-map__stage channel-map__stage--channel" aria-label="Channel">
          <h3>Channel</h3>
          <ul>
            {node({
              key: 'channel',
              selection: { kind: 'channel' },
              title: channel.definition.name,
              subtitle: 'Route events to agents',
              detail: `Version ${channel.version}`,
            })}
          </ul>
        </section>
        <section
          className="channel-map__stage channel-map__stage--consumers"
          aria-label="Consumers"
        >
          <h3>
            Consumers <span>{consumers.length}</span>
          </h3>
          <ul>{consumers.map(node)}</ul>
        </section>
      </div>
      <p className="channel-map__legend">
        Configured connections · Events follow the channel’s routing rules
      </p>
    </section>
  );
}
