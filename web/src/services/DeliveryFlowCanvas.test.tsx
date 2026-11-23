import { StrictMode } from 'react';
import { act, cleanup, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { DeliveryFlowCanvas } from './DeliveryFlowCanvas';
import type { ChannelNode } from './channelTopology';
import type { Edge, Node } from '@xyflow/react';

const { capture, flow } = vi.hoisted(() => ({
  capture: vi.fn(),
  flow: { fitView: vi.fn() },
}));
vi.mock('@xyflow/react', () => ({
  Background: () => null,
  Controls: () => null,
  Handle: () => null,
  Position: { Left: 'left', Right: 'right' },
  MarkerType: { ArrowClosed: 'arrowclosed' },
  useReactFlow: () => ({ fitView: flow.fitView }),
  ReactFlow: (props: { nodes: Node[]; edges: Edge[]; children: React.ReactNode }) => {
    capture(props);
    return <div>{props.children}</div>;
  },
}));
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  capture.mockClear();
  flow.fitView = vi.fn();
});
const node = (key: string, selection: ChannelNode['selection']): ChannelNode => ({
  key,
  selection,
  title: key,
  subtitle: key,
  detail: key,
});
const groups = [
  [node('source', { kind: 'producer', index: 0 })],
  [node('channel', { kind: 'channel' })],
  [node('agent', { kind: 'consumer', id: 'default' })],
];

it('shows directed routing motion, static tool associations, and accessible overlay content', () => {
  render(
    <DeliveryFlowCanvas
      groups={groups}
      toolsets={node('toolsets', { kind: 'channel' })}
      overlay={<button>Activity</button>}
    />,
  );
  const props = capture.mock.lastCall![0] as { nodes: Node[]; edges: Edge[] };
  expect(
    props.edges.filter((edge) => edge.animated).map(({ source, target }) => [source, target]),
  ).toEqual([
    ['source', 'channel'],
    ['channel', 'agent'],
  ]);
  expect(
    props.edges
      .filter((edge) => edge.source !== 'toolsets')
      .every(
        (edge) =>
          edge.markerEnd &&
          typeof edge.markerEnd !== 'string' &&
          edge.markerEnd.type === 'arrowclosed',
      ),
  ).toBe(true);
  expect(props.edges.find((edge) => edge.source === 'toolsets')).toMatchObject({
    target: 'agent',
    animated: false,
    label: 'Tools',
  });
  expect(props.edges.find((edge) => edge.source === 'toolsets')?.markerEnd).toBeUndefined();
  expect(props.nodes.find((value) => value.id === 'toolsets')!.position.y).toBeGreaterThan(
    props.nodes.find((value) => value.id === 'channel')!.position.y,
  );
  expect(
    screen.getByRole('button', { name: 'Activity' }).closest('.delivery-flow__overlay'),
  ).not.toBeNull();
});

it('keeps pending consumers visible without suggesting a tool connection', () => {
  const pending = { ...node('pending', { kind: 'consumer', id: 'pending' }), pending: true };
  render(
    <DeliveryFlowCanvas
      groups={[groups[0]!, groups[1]!, [...groups[2]!, pending]]}
      toolsets={node('toolsets', { kind: 'toolsets' })}
    />,
  );
  const props = capture.mock.lastCall![0] as { nodes: Node[]; edges: Edge[] };
  expect(props.nodes.some((value) => value.id === 'pending')).toBe(true);
  expect(
    props.edges.filter((edge) => edge.source === 'toolsets').map((edge) => edge.target),
  ).toEqual(['agent']);
});

it('responds to reduced-motion changes by leaving all arrows static', () => {
  let reduced = false;
  let notify = () => {};
  vi.stubGlobal('matchMedia', () => ({
    matches: reduced,
    addEventListener: (_: string, listener: () => void) => {
      notify = listener;
    },
    removeEventListener: vi.fn(),
  }));
  render(<DeliveryFlowCanvas groups={groups} />);
  expect(capture.mock.lastCall![0].edges.some((edge: Edge) => edge.animated)).toBe(true);
  act(() => {
    reduced = true;
    notify();
  });
  expect(
    capture.mock.lastCall![0].edges.every((edge: Edge) => !edge.animated && edge.markerEnd),
  ).toBe(true);
});

function mapMeasurements(initialWidth = 720) {
  let width = initialWidth;
  let height = 96;
  let reduced = false;
  let frame = 0;
  const frames = new Map<number, FrameRequestCallback>();
  const motionListeners = new Set<() => void>();
  const observers: Observer[] = [];
  class Observer implements ResizeObserver {
    readonly targets = new Set<Element>();
    constructor(readonly callback: ResizeObserverCallback) {
      observers.push(this);
    }
    observe = vi.fn((target: Element) => {
      this.targets.add(target);
    });
    unobserve = vi.fn((target: Element) => {
      this.targets.delete(target);
    });
    disconnect = vi.fn(() => this.targets.clear());
  }
  vi.stubGlobal('ResizeObserver', Observer);
  vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => {
    frames.set(++frame, callback);
    return frame;
  });
  vi.stubGlobal('cancelAnimationFrame', (id: number) => frames.delete(id));
  vi.spyOn(HTMLElement.prototype, 'clientWidth', 'get').mockImplementation(function (
    this: HTMLElement,
  ) {
    return this.classList.contains('delivery-flow') ? width : 0;
  });
  vi.spyOn(HTMLElement.prototype, 'clientHeight', 'get').mockImplementation(function (
    this: HTMLElement,
  ) {
    return this.classList.contains('delivery-flow__overlay') ? height : 0;
  });
  vi.stubGlobal('matchMedia', () => ({
    get matches() {
      return reduced;
    },
    addEventListener: (_: string, listener: () => void) => motionListeners.add(listener),
    removeEventListener: (_: string, listener: () => void) => motionListeners.delete(listener),
  }));
  const active = (target: Element) => observers.filter((observer) => observer.targets.has(target));
  return {
    observers,
    frames,
    active,
    resize(target: Element) {
      act(() => {
        for (const observer of active(target)) observer.callback([], observer);
      });
    },
    overlayHeight(value: number) {
      height = value;
    },
    width(value: number) {
      width = value;
    },
    motion(value: boolean) {
      act(() => {
        reduced = value;
        for (const listener of motionListeners) listener();
      });
    },
    flush() {
      const pending = [...frames.values()];
      frames.clear();
      act(() => {
        for (const callback of pending) callback(0);
      });
    },
  };
}

it('keeps resize subscriptions live while fitting the latest overlay and motion settings once', () => {
  const measurements = mapMeasurements();
  const { container } = render(
    <DeliveryFlowCanvas groups={groups} overlay={<button>Activity</button>} />,
  );
  const map = container.querySelector('.delivery-flow')!;
  const overlay = container.querySelector('.delivery-flow__overlay')!;
  measurements.resize(overlay);
  measurements.flush();
  expect(flow.fitView).toHaveBeenLastCalledWith({
    padding: { top: '144px', bottom: '64px', left: '32px', right: '32px' },
    maxZoom: 1,
    duration: 200,
  });
  const subscriptions = measurements.active(map);
  expect(subscriptions).toHaveLength(2);
  flow.fitView.mockClear();

  measurements.overlayHeight(157);
  measurements.resize(overlay);
  measurements.resize(map);
  measurements.motion(true);
  measurements.overlayHeight(176);
  measurements.resize(overlay);

  expect(measurements.active(map)).toEqual(subscriptions);
  for (const observer of subscriptions) expect(observer.disconnect).not.toHaveBeenCalled();
  expect(measurements.frames.size).toBe(1);
  measurements.flush();
  expect(flow.fitView).toHaveBeenCalledExactlyOnceWith({
    padding: { top: '224px', bottom: '64px', left: '32px', right: '32px' },
    maxZoom: 1,
    duration: 0,
  });
});

it('uses a replacement fit implementation without restarting resize observation', () => {
  const measurements = mapMeasurements();
  const { container, rerender } = render(<DeliveryFlowCanvas groups={groups} />);
  const map = container.querySelector('.delivery-flow')!;
  const subscriptions = measurements.active(map);
  measurements.flush();
  const previous = flow.fitView;
  expect(previous).toHaveBeenCalledTimes(1);

  flow.fitView = vi.fn();
  rerender(<DeliveryFlowCanvas groups={groups} />);
  expect(measurements.active(map)).toEqual(subscriptions);
  measurements.flush();
  expect(flow.fitView).toHaveBeenCalledExactlyOnceWith({
    padding: { top: '48px', bottom: '64px', left: '32px', right: '32px' },
    maxZoom: 1,
    duration: 200,
  });
  measurements.resize(map);
  measurements.flush();
  expect(flow.fitView).toHaveBeenCalledTimes(2);
  expect(previous).toHaveBeenCalledTimes(1);
});

it('fits initially without waiting for the first resize notification', () => {
  const measurements = mapMeasurements();
  render(<DeliveryFlowCanvas groups={groups} />);
  expect(measurements.frames.size).toBe(1);
  measurements.flush();
  expect(flow.fitView).toHaveBeenCalledTimes(1);
});

it('does not fit a hidden map and fits after its width becomes positive', () => {
  const measurements = mapMeasurements(0);
  const { container } = render(<DeliveryFlowCanvas groups={groups} />);
  measurements.flush();
  expect(flow.fitView).not.toHaveBeenCalled();
  measurements.width(720);
  measurements.resize(container.querySelector('.delivery-flow')!);
  measurements.flush();
  expect(flow.fitView).toHaveBeenCalledTimes(1);
});

it.each([false, true])(
  'cancels pending fits and releases observers on unmount (StrictMode=%s)',
  (strict) => {
    const measurements = mapMeasurements();
    const canvas = <DeliveryFlowCanvas groups={groups} overlay={<button>Activity</button>} />;
    const { container, unmount } = render(strict ? <StrictMode>{canvas}</StrictMode> : canvas);
    expect(measurements.frames.size).toBe(1);
    const map = container.querySelector('.delivery-flow')!;
    measurements.resize(map);
    measurements.resize(map);
    expect(measurements.frames.size).toBe(1);
    unmount();
    expect(measurements.frames.size).toBe(0);
    for (const observer of measurements.observers) {
      expect(observer.disconnect).toHaveBeenCalled();
      expect(observer.targets.size).toBe(0);
    }
    measurements.flush();
    expect(flow.fitView).not.toHaveBeenCalled();
  },
);
