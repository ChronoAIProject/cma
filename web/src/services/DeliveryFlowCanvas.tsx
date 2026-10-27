import { useEffect, useMemo, useRef, useSyncExternalStore, type RefObject } from 'react';
import {
  Background,
  Controls,
  Handle,
  Position,
  ReactFlow,
  useReactFlow,
  type Node,
  type NodeProps,
} from '@xyflow/react';
import { ChannelNodeIcon } from './ChannelNodeIcon';
import type { ChannelNode, ChannelSelection } from './channelTopology';
import '@xyflow/react/dist/style.css';

type CardData = Record<string, unknown> & {
  value: ChannelNode;
  selected: boolean;
  inspectorId?: string;
  onSelect?: (selection: ChannelSelection) => void;
};
type CardNode = Node<CardData, 'delivery'>;
function Card({ data }: NodeProps<CardNode>) {
  const { value } = data;
  const label = `${value.selection.kind === 'producer' ? 'Producer' : value.selection.kind === 'consumer' ? 'Consumer' : 'Channel'}: ${value.title} (${value.subtitle})`;
  const content = (
    <>
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
    </>
  );
  return (
    <>
      {value.selection.kind !== 'producer' ? (
        <Handle type="target" position={Position.Left} />
      ) : null}
      {data.onSelect ? (
        <button
          type="button"
          className="channel-map__node nodrag nopan"
          aria-label={label}
          aria-pressed={data.selected}
          aria-controls={data.inspectorId}
          data-kind={value.selection.kind}
          onClick={(event) => {
            event.stopPropagation();
            data.onSelect?.(value.selection);
          }}
        >
          {content}
        </button>
      ) : (
        <div
          className="channel-map__node channel-map__node--illustration"
          data-kind={value.selection.kind}
        >
          {content}
        </div>
      )}
      {value.selection.kind !== 'consumer' ? (
        <Handle type="source" position={Position.Right} />
      ) : null}
    </>
  );
}
const nodeTypes = { delivery: Card };
const motion = '(prefers-reduced-motion: reduce)';
const subscribe = (notify: () => void) => {
  const query = window.matchMedia(motion);
  query.addEventListener('change', notify);
  return () => query.removeEventListener('change', notify);
};

export function DeliveryFlowCanvas({
  groups,
  selected,
  onSelect,
  inspectorId,
  illustrative = false,
}: {
  readonly groups: readonly (readonly ChannelNode[])[];
  readonly selected?: string;
  readonly onSelect?: (selection: ChannelSelection) => void;
  readonly inspectorId?: string;
  readonly illustrative?: boolean;
}) {
  const container = useRef<HTMLDivElement>(null);
  const reduced = useSyncExternalStore(
    subscribe,
    () => window.matchMedia(motion).matches,
    () => false,
  );
  const nodes = useMemo<CardNode[]>(
    () =>
      groups.flatMap((group, column) =>
        group.map((value, row) => ({
          id: value.key,
          type: 'delivery',
          position: { x: column * 360, y: (row - (group.length - 1) / 2) * 120 },
          width: 240,
          height: 88,
          data: { value, selected: value.key === selected, onSelect, inspectorId },
        })),
      ),
    [groups, selected, onSelect, inspectorId],
  );
  const edges = useMemo(
    () =>
      groups.flatMap((group, column) =>
        column === 0
          ? []
          : (groups[column - 1] ?? []).flatMap((source) =>
              group.map((target) => ({
                id: `${source.key}:${target.key}`,
                source: source.key,
                target: target.key,
                type: 'smoothstep',
                animated: illustrative && !reduced,
              })),
            ),
      ),
    [groups, illustrative, reduced],
  );
  return (
    <div ref={container} className="delivery-flow" data-illustrative={illustrative || undefined}>
      <ReactFlow
        nodes={nodes}
        edges={edges}
        nodeTypes={nodeTypes}
        fitView
        fitViewOptions={{ padding: 0.2 }}
        minZoom={0.35}
        maxZoom={1.4}
        nodesDraggable={false}
        nodesConnectable={false}
        nodesFocusable={false}
        edgesFocusable={false}
        elementsSelectable={false}
        onNodeClick={onSelect ? (_event, node) => onSelect(node.data.value.selection) : undefined}
        panOnDrag={!illustrative}
        zoomOnScroll={!illustrative}
        zoomOnPinch={!illustrative}
        zoomOnDoubleClick={false}
        preventScrolling={!illustrative}
        ariaLabelConfig={{
          'controls.zoomIn.ariaLabel': 'Zoom in delivery map',
          'controls.zoomOut.ariaLabel': 'Zoom out delivery map',
          'controls.fitView.ariaLabel': 'Fit delivery map',
        }}
      >
        <ViewportFit container={container} reduced={reduced} />
        <Background gap={24} size={1} />
        {!illustrative ? <Controls showInteractive={false} /> : null}
      </ReactFlow>
    </div>
  );
}

function ViewportFit({
  container,
  reduced,
}: {
  readonly container: RefObject<HTMLDivElement | null>;
  readonly reduced: boolean;
}) {
  const { fitView } = useReactFlow();
  useEffect(() => {
    const element = container.current;
    if (!element) return;
    let frame = 0;
    const fit = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        if (element.clientWidth > 0)
          void fitView({ padding: 0.2, maxZoom: 1, duration: reduced ? 0 : 200 });
      });
    };
    const observer = new ResizeObserver(fit);
    observer.observe(element);
    fit();
    return () => {
      cancelAnimationFrame(frame);
      observer.disconnect();
    };
  }, [container, fitView, reduced]);
  return null;
}
