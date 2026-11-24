import {
  useEffect,
  useEffectEvent,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
  type RefObject,
  type ReactNode,
} from 'react';
import {
  Background,
  Controls,
  Handle,
  Position,
  MarkerType,
  ReactFlow,
  useReactFlow,
  type Node,
  type NodeProps,
  type Edge,
} from '@xyflow/react';
import { ChannelNodeIcon } from './ChannelNodeIcon';
import type { ChannelNode, ChannelSelection } from './channelTopology';
import '@xyflow/react/dist/style.css';

type CardData = Record<string, unknown> & {
  value: ChannelNode;
  selected: boolean;
  stacked: boolean;
  inspectorId?: string;
  onSelect?: (selection: ChannelSelection) => void;
};
type CardNode = Node<CardData, 'delivery'>;
function Card({ data }: NodeProps<CardNode>) {
  const { value } = data;
  const label = `${value.key === 'toolsets' ? 'Toolsets' : value.selection.kind === 'producer' ? 'Producer' : value.selection.kind === 'consumer' ? 'Consumer' : 'Channel'}: ${value.title} (${value.subtitle})`;
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
      {value.selection.kind !== 'producer' && value.key !== 'toolsets' ? (
        <Handle type="target" position={data.stacked ? Position.Top : Position.Left} />
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
        <Handle type="source" position={data.stacked ? Position.Bottom : Position.Right} />
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
  toolsets,
  overlay,
  selected,
  onSelect,
  inspectorId,
  illustrative = false,
}: {
  readonly groups: readonly (readonly ChannelNode[])[];
  readonly toolsets?: ChannelNode;
  readonly overlay?: ReactNode;
  readonly selected?: string;
  readonly onSelect?: (selection: ChannelSelection) => void;
  readonly inspectorId?: string;
  readonly illustrative?: boolean;
}) {
  const container = useRef<HTMLDivElement>(null);
  const [stacked, setStacked] = useState(false);
  useEffect(() => {
    const element = container.current;
    if (!element) return;
    const observer = new ResizeObserver(() => setStacked(element.clientWidth < 640));
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  const overlayElement = useRef<HTMLDivElement>(null);
  const [overlayHeight, setOverlayHeight] = useState(0);
  const hasOverlay = overlay !== undefined;
  useEffect(() => {
    const element = overlayElement.current;
    if (!element) return;
    const observer = new ResizeObserver(() => setOverlayHeight(element.clientHeight));
    observer.observe(element);
    return () => observer.disconnect();
  }, [hasOverlay]);
  const padding = useMemo(
    () => ({
      top: `${overlayHeight + 48}px` as const,
      bottom: '64px' as const,
      left: '32px' as const,
      right: '32px' as const,
    }),
    [overlayHeight],
  );
  const reduced = useSyncExternalStore(
    subscribe,
    () => window.matchMedia(motion).matches,
    () => false,
  );
  const nodes = useMemo<CardNode[]>(
    () => [
      ...groups.flatMap((group, column) =>
        group.map((value, row) => ({
          id: value.key,
          type: 'delivery' as const,
          position: stacked
            ? {
                x: 0,
                y:
                  (groups.slice(0, column).reduce((total, values) => total + values.length, 0) +
                    row +
                    (toolsets && column > 1 ? 1 : 0)) *
                  120,
              }
            : { x: column * 360, y: (row - (group.length - 1) / 2) * 120 },
          width: 240,
          height: 88,
          data: { value, stacked, selected: value.key === selected, onSelect, inspectorId },
        })),
      ),
      ...(toolsets
        ? [
            {
              id: toolsets.key,
              type: 'delivery' as const,
              position: stacked
                ? { x: 0, y: ((groups[0]?.length ?? 0) + (groups[1]?.length ?? 0)) * 120 }
                : { x: 360, y: Math.max(180, ((groups[1]?.length ?? 1) - 1) * 60 + 180) },
              width: 240,
              height: 88,
              data: {
                value: toolsets,
                stacked,
                selected: toolsets.key === selected,
                onSelect,
                inspectorId,
              },
            },
          ]
        : []),
    ],
    [groups, toolsets, selected, onSelect, inspectorId, stacked],
  );
  const edges = useMemo<Edge[]>(
    () => [
      ...groups.flatMap((group, column) =>
        column === 0
          ? []
          : (groups[column - 1] ?? []).flatMap((source) =>
              group.map((target) => ({
                id: `${source.key}:${target.key}`,
                source: source.key,
                target: target.key,
                type: 'smoothstep',
                animated: !reduced,
                markerEnd: {
                  type: MarkerType.ArrowClosed,
                  color: 'var(--dsw-alias-label-tertiary)',
                  width: 16,
                  height: 16,
                },
              })),
            ),
      ),
      ...(toolsets
        ? (groups[2] ?? [])
            .filter((target) => !target.pending)
            .map((target) => ({
              id: `${toolsets.key}:${target.key}`,
              source: toolsets.key,
              target: target.key,
              type: 'smoothstep',
              className: 'delivery-flow__tool-catalog',
              label: 'Tools',
              animated: false,
            }))
        : []),
    ],
    [groups, toolsets, reduced],
  );
  return (
    <div
      ref={container}
      className="delivery-flow"
      data-illustrative={illustrative || undefined}
      style={
        stacked && !illustrative
          ? { minHeight: overlayHeight + nodes.length * 120 + 120 }
          : undefined
      }
    >
      <ReactFlow
        nodes={nodes}
        edges={edges}
        nodeTypes={nodeTypes}
        fitView
        fitViewOptions={{ padding }}
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
        <ViewportFit container={container} reduced={reduced} padding={padding} />
        <Background gap={24} size={1} />
        {!illustrative ? <Controls showInteractive={false} fitViewOptions={{ padding }} /> : null}
      </ReactFlow>
      {overlay ? (
        <div ref={overlayElement} className="delivery-flow__overlay">
          {overlay}
        </div>
      ) : null}
    </div>
  );
}

function ViewportFit({
  container,
  reduced,
  padding,
}: {
  readonly container: RefObject<HTMLDivElement | null>;
  readonly reduced: boolean;
  readonly padding: {
    top: `${number}px`;
    bottom: `${number}px`;
    left: `${number}px`;
    right: `${number}px`;
  };
}) {
  const { fitView } = useReactFlow();
  const frame = useRef<number | null>(null);
  const fit = useEffectEvent(() => {
    frame.current = null;
    const element = container.current;
    if (element && element.clientWidth > 0)
      void fitView({ padding, maxZoom: 1, duration: reduced ? 0 : 200 });
  });
  const scheduleFit = useEffectEvent(() => {
    if (frame.current !== null) cancelAnimationFrame(frame.current);
    frame.current = requestAnimationFrame(() => fit());
  });
  useEffect(() => {
    const element = container.current;
    if (!element) return;
    const observer = new ResizeObserver(() => scheduleFit());
    observer.observe(element);
    return () => {
      if (frame.current !== null) cancelAnimationFrame(frame.current);
      frame.current = null;
      observer.disconnect();
    };
  }, [container]);
  useEffect(() => {
    scheduleFit();
  }, [padding, reduced, fitView]);
  return null;
}
